// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::config as image_assembly_config;
use crate::{
    config::{product_config::AssemblyInputBundle, FileEntry},
    util,
};
use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

type FileEntryMap = BTreeMap<String, PathBuf>;
type ConfigDataMap = BTreeMap<String, FileEntryMap>;

#[derive(Default)]
pub struct ImageAssemblyConfigBuilder {
    /// The base packages from the AssemblyInputBundles
    base: BTreeSet<PathBuf>,

    /// The cache packages from the AssemblyInputBundles
    cache: BTreeSet<PathBuf>,

    /// The system packages from the AssemblyInputBundles
    system: BTreeSet<PathBuf>,

    /// The boot_args from the AssemblyInputBundles
    boot_args: BTreeSet<String>,

    /// The bootfs_files from the AssemblyInputBundles
    bootfs_files: FileEntryMap,

    /// The config_data entries, by package and by destination path.
    config_data: ConfigDataMap,

    kernel_path: Option<PathBuf>,
    kernel_args: BTreeSet<String>,
    kernel_clock_backstop: Option<u64>,
}

impl ImageAssemblyConfigBuilder {
    /// Add an Assembly Input Bundle to the builder, via the path to its
    /// manifest.
    ///
    /// If any of the items it's trying to add are duplicates (either of itself
    /// or others, this will return an error.)
    pub fn add_bundle(&mut self, bundle_path: impl AsRef<Path>) -> Result<()> {
        let bundle = util::read_config(bundle_path.as_ref())?;

        // Strip filename from bundle path.
        let bundle_path = bundle_path.as_ref().parent().map(PathBuf::from).unwrap_or("".into());

        // Now add the parsed bundle
        self.add_parsed_bundle(bundle_path, bundle)
    }

    /// Add an Assembly Input Bundle to the builder, using a parsed
    /// AssemblyInputBundle, and the path to the folder that contains it.
    ///
    /// If any of the items it's trying to add are duplicates (either of itself
    /// or others, this will return an error.)
    pub fn add_parsed_bundle(
        &mut self,
        bundle_path: impl AsRef<Path>,
        bundle: AssemblyInputBundle,
    ) -> Result<()> {
        let bundle_path = bundle_path.as_ref();
        let AssemblyInputBundle { image_assembly: bundle, config_data } = bundle;

        Self::add_items_checking_for_duplicates(
            Self::paths_from(bundle_path, bundle.base),
            &mut self.base,
        )
        .context("base packages")?;

        Self::add_items_checking_for_duplicates(
            Self::paths_from(bundle_path, bundle.cache),
            &mut self.cache,
        )
        .context("cache packages")?;

        Self::add_items_checking_for_duplicates(
            Self::paths_from(bundle_path, bundle.system),
            &mut self.system,
        )
        .context("system packages")?;

        Self::add_items_checking_for_duplicates(bundle.boot_args, &mut self.boot_args)
            .context("boot_args")?;

        for FileEntry { destination, source } in
            Self::file_entry_paths_from(bundle_path, bundle.bootfs_files)
        {
            if self.bootfs_files.contains_key(&destination) {
                return Err(anyhow!("Duplicate bootfs destination path found: {}", &destination));
            } else {
                self.bootfs_files.insert(destination, source);
            }
        }

        if let Some(kernel) = bundle.kernel {
            util::set_option_once_or(
                &mut self.kernel_path,
                kernel.path.map(|p| bundle_path.join(p)),
                anyhow!("Only one input bundle can specify a kernel path"),
            )?;

            Self::add_items_checking_for_duplicates(kernel.args, &mut self.kernel_args)
                .context("Adding kernel args")?;

            util::set_option_once_or(
                &mut self.kernel_clock_backstop,
                kernel.clock_backstop,
                anyhow!("Only one input bundle can specify a kernel clock backstop"),
            )?;
        }

        for (package, entries) in config_data {
            for entry in Self::file_entry_paths_from(bundle_path, entries) {
                self.add_config_data_entry(&package, entry)?;
            }
        }
        Ok(())
    }

    fn paths_from<'a, I>(base: &'a Path, paths: I) -> impl IntoIterator<Item = PathBuf> + 'a
    where
        I: IntoIterator<Item = PathBuf> + 'a,
    {
        paths.into_iter().map(move |p| base.join(p))
    }

    fn file_entry_paths_from(
        base: &Path,
        entries: impl IntoIterator<Item = FileEntry>,
    ) -> Vec<FileEntry> {
        entries
            .into_iter()
            .map(|entry| FileEntry {
                destination: entry.destination,
                source: base.join(entry.source),
            })
            .collect()
    }

    /// For each item in `source`, add it to `dest` unless it's a duplicate of
    /// an item already in `dest`, in which case immediately stop and return an
    /// error.
    ///
    /// This _will_ alter the destination in error cases.  This is assumed to be
    /// ok as it is expected that the entire operation will be cancelled if an
    /// error is encountered.
    fn add_items_checking_for_duplicates<T: std::fmt::Debug + Ord>(
        source: impl IntoIterator<Item = T>,
        dest: &mut BTreeSet<T>,
    ) -> Result<()> {
        for item in source.into_iter() {
            if dest.contains(&item) {
                return Err(anyhow!("duplicate entry found: {:?}", item));
            } else {
                dest.insert(item);
            }
        }
        Ok(())
    }

    /// Add an entry to `config_data` for the given package.  If the entry
    /// duplicates an existing entry, return an error.
    fn add_config_data_entry(&mut self, package: impl AsRef<str>, entry: FileEntry) -> Result<()> {
        let package_entries = self.config_data.entry(package.as_ref().into()).or_default();
        if package_entries.contains_key(&entry.destination) {
            return Err(anyhow!(
                "Duplicate config_data destination path found: {}",
                &entry.destination
            ))
            .context(format!("Package: {}", package.as_ref()));
        } else {
            let FileEntry { destination, source } = entry;
            package_entries.insert(destination, source);
        }
        Ok(())
    }

    /// Construct an ImageAssembly ProductConfig from the collected items in the
    /// builder.
    ///
    /// If this cannot create a completed ProductConfig, it will return an error
    /// instead.
    pub fn build(self) -> Result<image_assembly_config::ProductConfig> {
        // Decompose the fields in self, so that they can be recomposed into the generated
        // image assembly configuration.
        let Self {
            base,
            cache,
            system,
            boot_args,
            bootfs_files,
            config_data: _,
            kernel_path,
            kernel_args,
            kernel_clock_backstop,
        } = self;

        // Construct a single "partial" config from the combined fields, and
        // then pass this to the ProductConfig::try_from_partials() to get the
        // final validation that it's complete.
        let partial = image_assembly_config::PartialProductConfig {
            system: system.into_iter().collect(),
            base: base.into_iter().collect(),
            cache: cache.into_iter().collect(),
            kernel: Some(image_assembly_config::PartialKernelConfig {
                path: kernel_path,
                args: kernel_args.into_iter().collect(),
                clock_backstop: kernel_clock_backstop,
            }),
            boot_args: boot_args.into_iter().collect(),
            bootfs_files: bootfs_files
                .into_iter()
                .map(|(destination, source)| FileEntry { destination, source })
                .collect(),
        };

        let image_assembly_config =
            image_assembly_config::ProductConfig::try_from_partials(std::iter::once(partial))?;

        Ok(image_assembly_config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_assembly_bundle() -> AssemblyInputBundle {
        let mut config_data = BTreeMap::<String, Vec<FileEntry>>::new();
        config_data.entry("base_package0".into()).or_default().push(FileEntry {
            source: "config/file/source/path".into(),
            destination: "config/file/dest".into(),
        });
        AssemblyInputBundle {
            image_assembly: image_assembly_config::PartialProductConfig {
                system: vec!["sys_package0".into()],
                base: vec!["base_package0".into()],
                cache: vec!["cache_package0".into()],
                kernel: Some(image_assembly_config::PartialKernelConfig {
                    path: Some("kernel/path".into()),
                    args: vec!["kernel_arg0".into()],
                    clock_backstop: Some(56244),
                }),
                boot_args: vec!["boot_arg0".into()],
                bootfs_files: vec![FileEntry {
                    source: "source/path/to/file".into(),
                    destination: "dest/file/path".into(),
                }],
            },
            config_data,
        }
    }

    #[test]
    fn test_builder() {
        let mut builder = ImageAssemblyConfigBuilder::default();
        builder.add_parsed_bundle(PathBuf::from("test"), make_test_assembly_bundle()).unwrap();
        let result: image_assembly_config::ProductConfig = builder.build().unwrap();

        assert_eq!(result.base, vec!(PathBuf::from("test/base_package0")));
        assert_eq!(result.cache, vec!(PathBuf::from("test/cache_package0")));
        assert_eq!(result.system, vec!(PathBuf::from("test/sys_package0")));

        assert_eq!(result.boot_args, vec!("boot_arg0".to_string()));
        assert_eq!(
            result.bootfs_files,
            vec!(FileEntry {
                source: "test/source/path/to/file".into(),
                destination: "dest/file/path".into()
            })
        );

        assert_eq!(result.kernel.path, PathBuf::from("test/kernel/path"));
        assert_eq!(result.kernel.args, vec!("kernel_arg0".to_string()));
        assert_eq!(result.kernel.clock_backstop, 56244);
    }

    /// Helper to duplicate the first item in an Vec<T: Clone> and make it also
    /// the last item. This intentionally panics if the Vec is empty.
    fn duplicate_first<T: Clone>(vec: &mut Vec<T>) {
        vec.push(vec.first().unwrap().clone());
    }

    #[test]
    fn test_builder_catches_dupe_base_pkgs_in_aib() {
        let mut aib = make_test_assembly_bundle();
        duplicate_first(&mut aib.image_assembly.base);

        let mut builder = ImageAssemblyConfigBuilder::default();
        assert!(builder.add_parsed_bundle(PathBuf::from("first"), aib).is_err());
    }

    #[test]
    fn test_builder_catches_dupe_cache_pkgs_in_aib() {
        let mut aib = make_test_assembly_bundle();
        duplicate_first(&mut aib.image_assembly.cache);

        let mut builder = ImageAssemblyConfigBuilder::default();
        assert!(builder.add_parsed_bundle(PathBuf::from("first"), aib).is_err());
    }

    #[test]
    fn test_builder_catches_dupe_system_pkgs_in_aib() {
        let mut aib = make_test_assembly_bundle();
        duplicate_first(&mut aib.image_assembly.system);

        let mut builder = ImageAssemblyConfigBuilder::default();
        assert!(builder.add_parsed_bundle(PathBuf::from("first"), aib).is_err());
    }

    fn test_duplicates_across_aibs_impl<
        T: Clone,
        F: Fn(&mut AssemblyInputBundle) -> &mut Vec<T>,
    >(
        accessor: F,
    ) {
        let mut aib = make_test_assembly_bundle();
        let mut second_aib = AssemblyInputBundle::default();

        let first_list = (accessor)(&mut aib);
        let second_list = (accessor)(&mut second_aib);

        // Clone the first item in the first AIB into the same list in the
        // second AIB to create a duplicate item across the two AIBs.
        let value = first_list.get(0).unwrap();
        second_list.push(value.clone());

        let mut builder = ImageAssemblyConfigBuilder::default();
        builder.add_parsed_bundle(PathBuf::from("first"), aib).unwrap();
        assert!(builder.add_parsed_bundle(PathBuf::from("second"), second_aib).is_err());
    }

    #[test]
    #[ignore] // As packages from different bundles have different paths,
              // this isn't currently working
    fn test_builder_catches_dupe_base_pkgs_across_aibs() {
        test_duplicates_across_aibs_impl(|a| &mut a.image_assembly.base);
    }

    #[test]
    #[ignore] // As packages from different bundles have different paths,
              // this isn't currently working
    fn test_builder_catches_dupe_cache_pkgs_across_aibs() {
        test_duplicates_across_aibs_impl(|a| &mut a.image_assembly.cache);
    }

    #[test]
    #[ignore] // As packages from different bundles have different paths,
              // this isn't currently working
    fn test_builder_catches_dupe_system_pkgs_across_aibs() {
        test_duplicates_across_aibs_impl(|a| &mut a.image_assembly.system);
    }

    #[test]
    fn test_builder_catches_dupe_bootfs_files_across_aibs() {
        test_duplicates_across_aibs_impl(|a| &mut a.image_assembly.bootfs_files);
    }

    #[test]
    fn test_builder_catches_dupe_config_data_across_aibs() {
        let aib = make_test_assembly_bundle();
        let mut second_aib = AssemblyInputBundle::default();

        let (key, vec) = aib.config_data.iter().next().unwrap();
        second_aib.config_data.entry(key.clone()).or_default().push(vec.get(0).unwrap().clone());

        let mut builder = ImageAssemblyConfigBuilder::default();
        builder.add_parsed_bundle(PathBuf::from("first"), aib).unwrap();
        assert!(builder.add_parsed_bundle(PathBuf::from("second"), second_aib).is_err());
    }
}
