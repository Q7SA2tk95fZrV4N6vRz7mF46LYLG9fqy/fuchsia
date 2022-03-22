// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod loader_cache;

use {
    fidl_fuchsia_test_runner as ftestrunner, fuchsia_async as fasync,
    fuchsia_component::server::ServiceFs,
    futures::prelude::*,
    log::{error, info, warn},
    thiserror::Error,
};

/// Run with 3 threads as all test runners will share the instance of this component.
/// We want to be able to serve `LibraryLoaderCacheBuilder` on one thread and `LibraryLoaderCache`
/// on other two threads.
#[fuchsia::component(threads = 3)]
async fn main() -> Result<(), anyhow::Error> {
    info!("started");
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(move |stream| {
        fasync::Task::spawn(async move {
            start_builder(stream).await.expect("failed to start lib loader builder.")
        })
        .detach();
    });
    fs.take_and_serve_directory_handle()?;
    fs.collect::<()>().await;
    Ok(())
}

/// Error encountered while calling fdio operations.
#[derive(Debug, Error)]
pub enum BuilderError {
    #[error("Cannot read request: {:?}", _0)]
    RequestRead(fidl::Error),
}

async fn start_builder(
    mut request_stream: ftestrunner::LibraryLoaderCacheBuilderRequestStream,
) -> Result<(), BuilderError> {
    while let Some(event) = request_stream.try_next().await.map_err(BuilderError::RequestRead)? {
        match event {
            ftestrunner::LibraryLoaderCacheBuilderRequest::Create {
                lib_directory,
                cache: server_end,
                ..
            } => {
                let lib_proxy = match lib_directory.into_proxy() {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("cannot convert lib directory to proxy: {:?}", e);
                        continue;
                    }
                };
                let cache = loader_cache::LibraryLoaderCache::new(lib_proxy.into());
                loader_cache::serve_cache(cache, server_end);
            }
        }
    }
    Ok(())
}