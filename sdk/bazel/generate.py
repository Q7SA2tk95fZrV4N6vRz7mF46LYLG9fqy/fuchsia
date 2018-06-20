#!/usr/bin/env python
# Copyright 2018 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import shutil
import stat
import sys

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
FUCHSIA_ROOT = os.path.dirname(  # $root
    os.path.dirname(             # scripts
    os.path.dirname(             # sdk
    SCRIPT_DIR)))                # bazel

sys.path += [os.path.join(FUCHSIA_ROOT, 'third_party', 'mako')]
from mako.template import Template
sys.path += [os.path.join(FUCHSIA_ROOT, 'scripts', 'sdk', 'common')]
from layout_builder import Builder, process_manifest


ARCH_MAP = {
    'arm64': 'aarch64',
    'x64': 'x86_64',
}


class Library(object):
    '''Represents a C/C++ library.
       Convenience storage object to be consumed by Mako templates.
       '''
    def __init__(self, name, target_arch):
        self.name = name
        self.srcs = []
        self.hdrs = []
        self.deps = []
        self.includes = []
        self.target_arch = target_arch


class FidlLibrary(object):
    '''Represents a FIDL library.
       Convenience storage object to be consumed by Mako templates.
       '''
    def __init__(self, name, library):
        self.name = name
        self.library = library
        self.srcs = []
        self.deps = []


class Arch(object):
    '''Represents an arch.
       Convenience storage object to be consumed by Mako templates.
       '''
    def __init__(self, arch):
        self.short_name = arch
        self.long_name = ARCH_MAP[arch]


class Crosstool(object):
    '''Represents available crosstools.
       Convenience storage object to be consumed by Mako templates.
       '''
    def __init__(self):
        self.arches = []


def remove_dashes(name):
    return name.replace('-', '_')


class BazelBuilder(Builder):

    def __init__(self, output, overlay):
        super(BazelBuilder, self).__init__(
            domains=['cpp', 'dart', 'exe', 'fidl'], ignored_domains=['image'])
        self.output = output
        self.is_overlay = overlay


    def source(self, *args):
        '''Builds a path in the current directory.'''
        return os.path.join(SCRIPT_DIR, *args)


    def dest(self, *args):
        '''Builds a path in the output directory.'''
        return os.path.join(self.output, *args)


    def write_file(self, path, template_name, data, append=False):
        '''Writes a file based on a Mako template.'''
        self.make_dir(path)
        template = Template(filename=self.source('templates',
                                                 template_name + '.mako'))
        with open(path, 'a' if append else 'w') as file:
            file.write(template.render(data=data))


    def prepare(self):
        if self.is_overlay:
            return
        # Copy the static files.
        shutil.copytree(self.source('base'), self.dest())


    def finalize(self):
        # Generate the crosstool.bzl based on the available sysroots.
        self.install_crosstool()
        if self.is_overlay:
            return
        # Write BUILD files for tools directories.
        tools_root = os.path.join(self.output, 'tools')
        for directory, _, _ in os.walk(tools_root, topdown=True):
            self.write_file(os.path.join(directory, 'BUILD'), 'tools', {})


    def install_crosstool(self):
        sysroot_dir = self.dest('arch')
        if os.path.isdir(sysroot_dir):
            crosstool = Crosstool()
            for arch in os.listdir(sysroot_dir):
                if arch in ARCH_MAP:
                    crosstool.arches.append(Arch(arch))
                else:
                    print('Unknown target arch: %s' % arch)
            self.write_file(self.dest('build_defs', 'crosstool.bzl'),
                            'crosstool_bzl', crosstool)
            self.write_file(self.dest('build_defs', 'BUILD.crosstool'),
                            'crosstool', crosstool)
            self.write_file(self.dest('build_defs', 'CROSSTOOL.in'),
                            'crosstool_in', crosstool)


    def install_dart_atom(self, atom):
        '''Installs an atom from the "dart" domain.'''
        type = atom.tags['type']
        if type == 'library':
            self.install_dart_library_atom(atom)
        elif type == 'tool':
            self.install_dart_tool_atom(atom)
        else:
            print('Dart atom type "%s" not handled, skipping %s.'
                  % (type, atom.id))


    def install_dart_library_atom(self, atom):
        base = self.dest('dart', remove_dashes(atom.id.name))
        for file in atom.files:
            dest = self.make_dir(os.path.join(base, file.destination))
            shutil.copy2(file.source, dest)
            # TODO(pylaligand): add a build file.


    def install_dart_tool_atom(self, atom):
        if self.is_overlay:
            return
        for file in atom.files:
            dest = self.make_dir(self.dest('tools', file.destination))
            shutil.copy2(file.source, dest)


    def install_cpp_atom(self, atom):
        '''Installs an atom from the "cpp" domain.'''
        type = atom.tags['type']
        if type == 'compiled_shared':
            self.install_cpp_prebuilt_atom(atom)
        elif type == 'sources':
            self.install_cpp_source_atom(atom)
        elif type == 'sysroot':
            self.install_cpp_sysroot_atom(atom)
        else:
            print('C++ atom type "%s" not handled, skipping %s.'
                  % (type, atom.id))


    def install_cpp_prebuilt_atom(self, atom, check_arch=True):
        '''Installs a prebuilt atom from the "cpp" domain.'''
        if check_arch and atom.tags['arch'] != 'target':
            print('Only libraries compiled for a target are supported, '
                  'skipping %s.' % atom.id)
            return

        name = remove_dashes(atom.id.name)
        library = Library(name, self.metadata.target_arch)
        base = self.dest('pkg', name)

        for file in atom.files:
            destination = file.destination
            extension = os.path.splitext(destination)[1][1:]
            if extension == 'so' or extension == 'o':
                dest = os.path.join(base, 'arch',
                                    self.metadata.target_arch, destination)
                if os.path.isfile(dest):
                    raise Exception('File already exists: %s.' % dest)
                self.make_dir(dest)
                shutil.copy2(file.source, dest)
                if extension == 'so' and destination.startswith('lib'):
                    src = os.path.join('arch', self.metadata.target_arch,
                                       destination)
                    library.srcs.append(src)
            elif self.is_overlay:
                # Only binaries get installed in overlay mode.
                continue
            elif (extension == 'h' or extension == 'modulemap' or
                    extension == 'inc' or extension == 'rs'):
                dest = self.make_dir(os.path.join(base, destination))
                shutil.copy2(file.source, dest)
                if extension == 'h':
                    library.hdrs.append(destination)
            else:
                raise Exception('Error: unknow file extension "%s" for %s.' %
                                (extension, atom.id))

        # Only write the prebuilt library BUILD top when not in overlay.
        if not self.is_overlay:
            for dep_id in atom.deps:
                library.deps.append('//pkg/' + remove_dashes(dep_id.name))
            library.includes.append('include')
            self.write_file(os.path.join(base, 'BUILD'),
                            'cc_prebuilt_library_top', library)

        # Write the arch specific dependencies.
        self.write_file(os.path.join(base, 'BUILD'),
                        'cc_prebuilt_library_srcs', library, append=True)


    def install_cpp_source_atom(self, atom):
        '''Installs a source atom from the "cpp" domain.'''
        if self.is_overlay:
            return

        name = remove_dashes(atom.id.name)
        library = Library(name, self.metadata.target_arch)
        base = self.dest('pkg', name)

        for file in atom.files:
            dest = self.make_dir(os.path.join(base, file.destination))
            shutil.copy2(file.source, dest)
            extension = os.path.splitext(file.destination)[1][1:]
            if extension == 'h':
                library.hdrs.append(file.destination)
            elif extension == 'c' or extension == 'cc' or extension == 'cpp':
                library.srcs.append(file.destination)
            else:
                raise Exception('Error: unknow file extension "%s" for %s.' %
                                (extension, atom.id))

        for dep_id in atom.deps:
            library.deps.append('//pkg/' + remove_dashes(dep_id.name))

        library.includes.append('include')

        self.write_file(os.path.join(base, 'BUILD'), 'cc_library', library)


    def install_cpp_sysroot_atom(self, atom):
        '''Installs a sysroot atom from the "cpp" domain.'''
        base = self.dest('arch', self.metadata.target_arch, 'sysroot')
        for file in atom.files:
            dest = self.make_dir(os.path.join(base, file.destination))
            shutil.copy2(file.source, dest)
        self.write_file(os.path.join(base, 'BUILD'), 'sysroot', [])


    def install_exe_atom(self, atom):
        '''Installs an atom from the "exe" domain.'''
        if atom.tags['arch'] != 'host':
            print('Only host executables are supported, skipping %s.' % atom.id)
            return
        if self.is_overlay:
            return
        files = atom.files
        if len(files) != 1:
            raise Exception('Error: executable with multiple files: %s.'
                            % atom.id)
        file = files[0]
        destination = self.make_dir(self.dest('tools', file.destination))
        shutil.copy2(file.source, destination)


    def install_fidl_atom(self, atom):
        '''Installs an atom from the "fidl" domain.'''
        if self.is_overlay:
            return
        name = remove_dashes(atom.id.name)
        data = FidlLibrary(name, atom.tags['name'])
        base = self.dest('fidl', name)
        for file in atom.files:
            dest = self.make_dir(os.path.join(base, file.destination))
            shutil.copy2(file.source, dest)
            data.srcs.append(file.destination)
        for dep_id in atom.deps:
            data.deps.append('//fidl/' + remove_dashes(dep_id.name))
        self.write_file(os.path.join(base, 'BUILD'), 'fidl', data)


def main():
    parser = argparse.ArgumentParser(
            description=('Lays out an SDK based on the given manifest'))
    parser.add_argument('--manifest',
                        help='Path to the SDK manifest',
                        required=True)
    parser.add_argument('--output',
                        help='Path to the directory where to install the SDK',
                        required=True)
    parser.add_argument('--overlay',
                        help='Whether to overlay target binaries on top of an '
                             'existing layout',
                        action='store_true')
    args = parser.parse_args()

    # Remove any existing output.
    if args.overlay:
        if not os.path.isdir(args.output) :
            print('Cannot overlay on top of missing output directory: %s.' %
                  args.output)
            return 1
    else:
        shutil.rmtree(args.output, True)

    builder = BazelBuilder(args.output, args.overlay)
    return 0 if process_manifest(args.manifest, builder) else 1


if __name__ == '__main__':
    sys.exit(main())
