// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package project

import (
	"io/fs"
	"path/filepath"
)

var AllProjects map[string]*Project

func init() {
	AllProjects = make(map[string]*Project)
}

func Initialize(c *ProjectConfig) error {
	Config = c
	return initializeReadmes()
}

// Projects are created using README.fuchsia files.
// Many projects in the fuchsia tree do not have a README.fuchsia file,
// or they are incorrectly formatted.
//
// You can setup custom README.fuchsia files in a special directory,
// point to them in the config file, and they'll be parsed here
// before the rest of check-licenses executes.
func initializeReadmes() error {
	for _, readmeCategory := range Config.Readmes {
		for _, readmePath := range readmeCategory.Paths {
			readmePath = filepath.Join(Config.FuchsiaDir, readmePath)
			if err := filepath.WalkDir(readmePath, func(currentPath string, info fs.DirEntry, err error) error {
				if err != nil {
					return err
				}

				if info.Name() == "README.fuchsia" {
					plusVal(NumInitCustomProjects, currentPath)
					projectRoot := filepath.Dir(currentPath)
					projectRoot, err = filepath.Rel(readmePath, projectRoot)
					if err != nil {
						return err
					}

					// The root project for the Fuchsia tree is special, since "currentPath" will be an empty string.
					if projectRoot == "." {
						projectRoot = Config.FuchsiaDir
					}

					if p, err := NewProject(currentPath, projectRoot); err != nil {
						return err
					} else {
						AllProjects[projectRoot] = p
					}
				}
				return nil
			}); err != nil {
				return err
			}
		}
	}
	return nil
}