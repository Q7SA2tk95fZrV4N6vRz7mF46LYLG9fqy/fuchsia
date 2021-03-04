// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use {
    super::{BlobNetworkTimeouts, FetchError, FetchStats},
    anyhow::anyhow,
    fuchsia_async::TimeoutExt as _,
    fuchsia_syslog::fx_log_warn,
    futures::{
        future::TryFutureExt as _,
        stream::{Stream, TryStreamExt as _},
    },
    hyper::{body::HttpBody, Body, Request, StatusCode},
    std::{
        convert::{TryFrom, TryInto as _},
        str::FromStr,
        sync::Arc,
    },
};

// It should be impossible to infinite loop on resumption requests because we only attempt
// to resume when progress was made on the previous GET. This is a backstop in case of
// a logic bug.
const MAX_RESUMPTION_ATTEMPTS: u64 = 50u64;

// On success, returns the Content-Length of the resource, as determined by the first GET,
// and a Stream of the content.
pub(super) async fn resuming_get<'a>(
    client: &'a fuchsia_hyper::HttpsClient,
    uri: &'a http::Uri,
    expected_len: Option<u64>,
    blob_network_timeouts: BlobNetworkTimeouts,
    fetch_stats: Arc<FetchStats>,
) -> Result<(u64, impl Stream<Item = Result<hyper::body::Bytes, FetchError>> + 'a), FetchError> {
    let request = Request::get(uri)
        .body(Body::empty())
        .map_err(|e| FetchError::Http { e, uri: uri.to_string() })?;
    let response = client
        .request(request)
        .map_err(|e| FetchError::Hyper { e, uri: uri.to_string() })
        .on_timeout(blob_network_timeouts.header(), || {
            Err(FetchError::BlobHeaderTimeout { uri: uri.to_string() })
        })
        .await?;

    if response.status() != StatusCode::OK {
        return Err(FetchError::BadHttpStatus { code: response.status(), uri: uri.to_string() });
    }

    let expected_len = match (expected_len, response.size_hint().exact()) {
        (Some(expected), Some(actual)) => {
            if expected != actual {
                return Err(FetchError::ContentLengthMismatch {
                    expected,
                    actual,
                    uri: uri.to_string(),
                });
            } else {
                expected
            }
        }
        (Some(length), None) | (None, Some(length)) => length,
        (None, None) => return Err(FetchError::UnknownLength { uri: uri.to_string() }),
    };

    let chunks = async_generator::generate(move |mut co| {
        async move {
            let mut bytes_downloaded = 0;
            let mut progress_this_attempt = false;
            let mut chunks = response.into_body();
            while bytes_downloaded < expected_len {
                match chunks
                    .try_next()
                    .map_err(|e| FetchError::Hyper { e, uri: uri.to_string() })
                    .on_timeout(blob_network_timeouts.body(), || {
                        Err(FetchError::BlobBodyTimeout { uri: uri.to_string() })
                    })
                    .await
                {
                    Ok(Some(chunk)) => {
                        if chunk.len() > 0 {
                            progress_this_attempt = true;
                            bytes_downloaded += chunk.len() as u64;
                            co.yield_(chunk).await;
                        }
                    }
                    Ok(None) => break,
                    Err(e) if progress_this_attempt => {
                        fx_log_warn!(
                            "Resuming failed blob GET after partial success. \
                            resumptions: {}. downloaded {} of {}. error: {:#}",
                            fetch_stats.resumptions(), bytes_downloaded, expected_len, anyhow!(e)
                        );
                        progress_this_attempt = false;
                        fetch_stats.resume();
                        if fetch_stats.resumptions() >= MAX_RESUMPTION_ATTEMPTS {
                            return Err(FetchError::ExceededResumptionAttemptLimit{
                                uri: uri.to_string(),
                                limit: MAX_RESUMPTION_ATTEMPTS
                            });
                        }
                        let first_byte_pos = bytes_downloaded;
                        // This will never overflow, because expected_len is const and if
                        // expected_len is 0 then this while loop can never be entered.
                        let last_byte_pos = expected_len - 1;
                        // TODO(fxbug.dev/71333) use open-ended range ("bytes={}-")
                        let request = Request::get(uri)
                            .header(
                                http::header::RANGE,
                                format!("bytes={}-{}", first_byte_pos, last_byte_pos)
                            )
                            .body(Body::empty())
                            .map_err(|e| FetchError::Http { e, uri: uri.to_string() })?;

                        let response = client
                            .request(request)
                            .map_err(|e| FetchError::Hyper { e, uri: uri.to_string() })
                            .on_timeout(blob_network_timeouts.header(), || {
                                Err(FetchError::BlobHeaderTimeout { uri: uri.to_string() })
                            })
                            .await?;

                        if response.status() != StatusCode::PARTIAL_CONTENT {
                            return
                                Err(FetchError::ExpectedHttpStatus206 {
                                    code: response.status(),
                                    uri: uri.to_string(),
                                });
                        }

                        if let Some(content_range) = response.headers().get(
                            http::header::CONTENT_RANGE
                        ) {
                            let content_range = content_range.try_into().map_err(
                                |e| FetchError::MalformedContentRangeHeader {
                                     e,
                                     uri: uri.to_string(),
                                     first_byte_pos,
                                     last_byte_pos,
                                     header: content_range.clone(),
                                }
                            )?;

                            let expected = HttpContentRange {
                                range: Some((first_byte_pos, last_byte_pos)),
                                len: Some(expected_len)
                            };
                            if content_range != expected {
                                return Err(FetchError::InvalidContentRangeHeader {
                                    uri: uri.to_string(),
                                    content_range,
                                    expected
                                });
                            }

                            if let Some(content_length) = HttpBody::size_hint(response.body()).exact() {
                                if content_length != 1 + last_byte_pos - first_byte_pos {
                                    return Err(FetchError::ContentLengthContentRangeMismatch{
                                        uri: uri.to_string(), content_length, content_range
                                    });
                                }
                            }
                        } else {
                            return
                                Err(FetchError::MissingContentRangeHeader {
                                    uri: uri.to_string(),
                                });
                        }

                        chunks = response.into_body();
                    }
                    Err(e) /* !progress_this_attempt */=> {
                        return Err(e);
                    }
                }
            }
            Ok(())
        }
    });

    Ok((expected_len, chunks.into_try_stream()))
}

/// An http Content-Range header, e.g. "bytes 0-499/1234"
/// https://tools.ietf.org/html/rfc7233#section-4.2
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HttpContentRange {
    range: Option<(u64, u64)>,
    len: Option<u64>,
}

fn split_pair(s: &str, sep: char) -> Option<(&str, &str)> {
    let mut iter = s.splitn(2, sep).fuse();
    match (iter.next(), iter.next()) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    }
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum ContentRangeParseError {
    #[error("complete length")]
    CompleteLength,
    #[error("first byte position")]
    FirstBytePos,
    #[error("last byte position")]
    LastBytePos,
    #[error("invalid byte range")]
    InvalidByteRange,
    #[error("malformed header")]
    MalformedHeader,
    #[error("unsupported range unit")]
    UnsupportedRangeUnit,
    #[error("invalid utf8")]
    InvalidUtf8,
}

impl TryFrom<&http::HeaderValue> for HttpContentRange {
    type Error = ContentRangeParseError;
    fn try_from(value: &http::HeaderValue) -> Result<Self, Self::Error> {
        value.to_str().map_err(|_| ContentRangeParseError::InvalidUtf8)?.parse()
    }
}

impl FromStr for HttpContentRange {
    type Err = ContentRangeParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match split_pair(s, ' ') {
            Some(("bytes", etc)) => {
                let (range, len) = match split_pair(etc, '/') {
                    Some(("*", "*")) => (None, None),
                    Some(("*", len)) => {
                        let len =
                            len.parse().map_err(|_| ContentRangeParseError::CompleteLength)?;
                        (None, Some(len))
                    }
                    Some((range, len)) => {
                        let range = match split_pair(range, '-') {
                            Some((a, b)) => {
                                let a =
                                    a.parse().map_err(|_| ContentRangeParseError::FirstBytePos)?;
                                let b =
                                    b.parse().map_err(|_| ContentRangeParseError::LastBytePos)?;
                                if b < a {
                                    return Err(ContentRangeParseError::InvalidByteRange);
                                }
                                Some((a, b))
                            }
                            _ => return Err(ContentRangeParseError::InvalidByteRange),
                        };
                        let len = if len == "*" {
                            None
                        } else {
                            Some(len.parse().map_err(|_| ContentRangeParseError::CompleteLength)?)
                        };
                        (range, len)
                    }
                    _ => return Err(ContentRangeParseError::MalformedHeader),
                };

                return Ok(HttpContentRange { range, len });
            }
            _ => return Err(ContentRangeParseError::UnsupportedRangeUnit),
        }
    }
}

#[cfg(test)]
mod tests {
    use {super::*, matches::assert_matches};

    #[test]
    fn parse_http_byte_range_success() {
        let cases = vec![
            ("bytes 42-1233/1234", Some((42, 1233)), Some(1234)),
            ("bytes 42-1233/*", Some((42, 1233)), None),
            ("bytes */1231", None, Some(1231)),
            ("bytes */*", None, None),
        ];

        for (input, range, len) in cases {
            assert_eq!(HttpContentRange::from_str(input).unwrap(), HttpContentRange { range, len });
        }
    }

    #[test]
    fn parse_http_byte_range_failure() {
        use ContentRangeParseError::*;
        let cases = vec![
            ("", UnsupportedRangeUnit),
            ("nonunit 52-1233/1234", UnsupportedRangeUnit),
            ("byte", UnsupportedRangeUnit),
            ("byte ", UnsupportedRangeUnit),
            ("bytes", UnsupportedRangeUnit),
            ("bytes ", MalformedHeader),
            ("bytes invalid", MalformedHeader),
            ("bytes 1", MalformedHeader),
            ("bytes 1-", MalformedHeader),
            ("bytes 1-2", MalformedHeader),
            ("bytes 1-2/", CompleteLength),
            ("bytes 1/", InvalidByteRange),
            ("bytes 1-/", LastBytePos),
            ("bytes /", InvalidByteRange),
            ("bytes -", MalformedHeader),
            ("bytes -/", FirstBytePos),
            ("bytes -1/", FirstBytePos),
            ("bytes 1-/2", LastBytePos),
            ("bytes a-b/c", FirstBytePos),
            ("bytes a-10/100", FirstBytePos),
            ("bytes 10-b/100", LastBytePos),
            ("bytes 10-100/c", CompleteLength),
            ("bytes 10-4/*", InvalidByteRange),
        ];

        for (input, error) in cases {
            assert_matches!(HttpContentRange::from_str(input), Err(e) if e == error);
        }
    }
}
