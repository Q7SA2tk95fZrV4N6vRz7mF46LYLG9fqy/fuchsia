// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use {
    crate::repository::{PmRepository, Repository, RepositoryKeyConfig},
    anyhow::{anyhow, Context, Result},
    camino::Utf8PathBuf,
    fuchsia_pkg::PackageBuilder,
    futures::io::AllowStdIo,
    maplit::hashmap,
    std::{
        fs::{copy, create_dir, create_dir_all, File},
        path::{Path, PathBuf},
    },
    tuf::{
        crypto::Ed25519PrivateKey, interchange::Json, metadata::TargetPath,
        repo_builder::RepoBuilder, repository::FileSystemRepositoryBuilder,
    },
    walkdir::WalkDir,
};

const EMPTY_REPO_PATH: &str = "host_x64/test_data/ffx_lib_pkg/empty-repo";

#[cfg(test)]
pub(crate) const PKG1_HASH: &str =
    "2881455493b5870aaea36537d70a2adc635f516ac2092598f4b6056dabc6b25d";

#[cfg(test)]
pub(crate) const PKG2_HASH: &str =
    "050907f009ff634f9aa57bff541fb9e9c2c62b587c23578e77637cda3bd69458";

#[cfg(test)]
pub(crate) const PKG1_BIN_HASH: &str =
    "72e1e7a504f32edf4f23e7e8a3542c1d77d12541142261cfe272decfa75f542d";

#[cfg(test)]
pub(crate) const PKG1_LIB_HASH: &str =
    "8a8a5f07f935a4e8e1fd1a1eda39da09bb2438ec0adfb149679ddd6e7e1fbb4f";

pub fn repo_key() -> RepositoryKeyConfig {
    RepositoryKeyConfig::Ed25519Key(
        [
            29, 76, 86, 76, 184, 70, 108, 73, 249, 127, 4, 47, 95, 63, 36, 35, 101, 255, 212, 33,
            10, 154, 26, 130, 117, 157, 125, 88, 175, 214, 109, 113,
        ]
        .to_vec(),
    )
}

pub fn repo_private_key() -> Ed25519PrivateKey {
    Ed25519PrivateKey::from_ed25519(&[
        80, 121, 161, 145, 5, 165, 178, 98, 248, 146, 132, 195, 60, 32, 72, 122, 150, 223, 124,
        216, 217, 43, 74, 9, 221, 38, 156, 113, 181, 63, 234, 98, 190, 11, 152, 63, 115, 150, 218,
        103, 92, 64, 198, 185, 62, 71, 252, 237, 124, 30, 158, 168, 163, 42, 31, 233, 82, 186, 143,
        81, 151, 96, 179, 7,
    ])
    .unwrap()
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    let walker = WalkDir::new(from);
    for entry in walker.into_iter() {
        let entry = entry?;
        let to_path = to.join(entry.path().strip_prefix(from)?);
        if entry.metadata()?.is_dir() {
            create_dir(&to_path).context(format!("creating {:?}", to_path))?;
        } else {
            copy(entry.path(), &to_path).context(format!(
                "copying {:?} to {:?}",
                entry.path(),
                to_path
            ))?;
        }
    }

    Ok(())
}

pub async fn make_readonly_empty_repository(name: &str) -> Result<Repository> {
    let backend = PmRepository::new(Utf8PathBuf::from(EMPTY_REPO_PATH));
    Repository::new(name, Box::new(backend)).await.map_err(|e| anyhow!(e))
}

pub async fn make_writable_empty_repository(name: &str, root: Utf8PathBuf) -> Result<Repository> {
    let src = PathBuf::from(EMPTY_REPO_PATH).canonicalize()?;
    copy_dir(&src, root.as_std_path())?;
    let backend = PmRepository::new(root);
    Ok(Repository::new(name, Box::new(backend)).await?)
}

pub async fn make_repository(metadata_dir: &Path, blobs_dir: &Path) {
    create_dir_all(&metadata_dir).unwrap();
    create_dir_all(&blobs_dir).unwrap();

    // Construct some packages for the repository.
    let build_tmp = tempfile::tempdir().unwrap();
    let build_path = build_tmp.path();

    let packages = ["package1", "package2"].map(|name| {
        let package_path = build_path.join(name);

        let mut builder = PackageBuilder::new(name);
        builder
            .add_contents_as_blob(
                format!("bin/{}", name),
                format!("binary {}", name).as_bytes(),
                &package_path,
            )
            .unwrap();
        builder
            .add_contents_as_blob(
                format!("lib/{}", name),
                format!("lib {}", name).as_bytes(),
                &package_path,
            )
            .unwrap();
        builder
            .add_contents_to_far(
                format!("meta/{}.cm", name),
                format!("cm {}", name).as_bytes(),
                &package_path,
            )
            .unwrap();
        builder
            .add_contents_to_far(
                format!("meta/{}.cmx", name),
                format!("cmx {}", name).as_bytes(),
                &package_path,
            )
            .unwrap();

        let meta_far_path = package_path.join("meta.far");
        let manifest = builder.build(&package_path, &meta_far_path).unwrap();

        // Copy the package blobs into the blobs directory.
        let mut meta_far_merkle = None;
        for blob in manifest.blobs() {
            let merkle = blob.merkle.to_string();

            if blob.path == "meta/" {
                meta_far_merkle = Some(merkle.clone());
            }

            let mut src = std::fs::File::open(&blob.source_path).unwrap();
            let mut dst = std::fs::File::create(blobs_dir.join(merkle)).unwrap();
            std::io::copy(&mut src, &mut dst).unwrap();
        }

        (name, meta_far_path, meta_far_merkle.unwrap())
    });

    // Write TUF metadata
    let repo = FileSystemRepositoryBuilder::<Json>::new(metadata_dir)
        .targets_prefix("targets")
        .build()
        .unwrap();

    let key = repo_private_key();
    let mut builder = RepoBuilder::create(repo)
        .trusted_root_keys(&[&key])
        .trusted_targets_keys(&[&key])
        .trusted_snapshot_keys(&[&key])
        .trusted_timestamp_keys(&[&key])
        .stage_root()
        .unwrap();

    // Add all the packages to the metadata.
    for (name, meta_far_path, meta_far_merkle) in packages {
        builder = builder
            .add_target_with_custom(
                TargetPath::new(name).unwrap(),
                AllowStdIo::new(File::open(meta_far_path).unwrap()),
                hashmap! { "merkle".into() => meta_far_merkle.into() },
            )
            .await
            .unwrap();
    }

    builder.commit().await.unwrap();
}

pub async fn make_pm_repository(name: &str, dir: impl Into<Utf8PathBuf>) -> Repository {
    let dir = dir.into();
    let metadata_dir = dir.join("repository");
    let blobs_dir = metadata_dir.join("blobs");
    make_repository(metadata_dir.as_std_path(), blobs_dir.as_std_path()).await;

    let backend = PmRepository::new(dir);
    Repository::new(name, Box::new(backend)).await.unwrap()
}
