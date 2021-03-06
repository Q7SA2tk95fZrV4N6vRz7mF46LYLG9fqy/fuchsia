// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! See the `README.md` file in this directory for more detail.

#[cfg(test)]
mod tests {

    use anyhow::Error;
    use fuchsia_async as fasync;
    use fuchsia_syslog;
    use tests_intl_timezone;

    #[fasync::run_singlethreaded(test)]
    async fn check_reported_time_in_flutter_vm() -> Result<(), Error> {
        fuchsia_syslog::init_with_tags(&[
            "e2e",
            "timezone",
            "flutter",
            "check_reported_time_in_flutter_vm",
        ])
        .unwrap();
        tests_intl_timezone::check_reported_time_with_update(/*get_view=*/ true).await
    }
} // tests

// Makes fargo happy.
fn main() {}
