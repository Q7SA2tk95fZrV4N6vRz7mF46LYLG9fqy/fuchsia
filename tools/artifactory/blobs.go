// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package artifactory

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"

	pm "go.fuchsia.dev/fuchsia/src/sys/pkg/bin/pm/build"
	tools "go.fuchsia.dev/fuchsia/tools/build"
)

// BlobsUpload parses all the package manifests in the build and returns an
// Upload for all the blobs.
func BlobsUpload(mods *tools.Modules, destination string) (Upload, error) {
	return blobsUpload(mods, destination)
}

func blobsUpload(mods pkgManifestsModules, destination string) (Upload, error) {
	// Obtain the absolute paths.
	packageManifestList := filepath.Join(mods.BuildDir(), mods.PackageManifestsLocation()[0])
	manifests, err := tools.LoadPackageManifests(packageManifestList)
	// It's ok for package manifests to not exist, this happens on e.g. SDK-only builders.
	if _, err := os.Stat(packageManifestList); os.IsNotExist(err) {
		return Upload{
			Compress:    true,
			Contents:    []byte("[]"),
			Destination: destination,
		}, nil
	}
	if err != nil {
		return Upload{}, fmt.Errorf("failed to load list of package manifests: %s", err)
	}
	absolutePaths := make([]string, len(manifests))
	for _, path := range manifests {
		absolutePaths = append(absolutePaths, filepath.Join(mods.BuildDir(), path))
	}

	// Obtain a list of blobs.
	blobs, err := loadBlobsFromPackageManifests(absolutePaths)
	if err != nil {
		return Upload{}, fmt.Errorf("failed to parse blobs: %w", err)
	}

	// Convert blobs list to json.
	blobsJSON, err := json.Marshal(blobs)
	if err != nil {
		return Upload{}, fmt.Errorf("failed to marshal blobs to json: %w", err)
	}

	return Upload{
		Compress:    true,
		Contents:    blobsJSON,
		Destination: destination,
	}, nil
}

// loadBlobsFromPackageManifests collects the blobs from the provided package manifests.
func loadBlobsFromPackageManifests(paths []string) ([]pm.PackageBlobInfo, error) {
	// The same blob might appear in multiple package manifests.
	seen := map[pm.PackageBlobInfo]bool{}
	for _, path := range paths {
		// It's ok for package manifests to not exist.
		if _, err := os.Stat(path); os.IsNotExist(err) {
			continue
		}

		// Collect the blobs from this package manifest.
		pkgManifest, err := pm.LoadPackageManifest(path)
		if err != nil {
			return nil, fmt.Errorf("failed to parse package manifest: %w", err)
		}
		for _, blob := range pkgManifest.Blobs {
			if _, ok := seen[blob]; !ok {
				seen[blob] = true
			}
		}
	}

	// Convert back to a regular list, sort by hash so it's deterministic.
	var blobs []pm.PackageBlobInfo
	for blob := range seen {
		blobs = append(blobs, blob)
	}
	sort.Slice(blobs, func(i, j int) bool {
		return blobs[i].Merkle.String() < blobs[j].Merkle.String()
	})

	return blobs, nil
}

type pkgManifestsModules interface {
	BuildDir() string
	PackageManifestsLocation() []string
}
