//! Browser startup, download metadata, and atomic import policies.

/// Query flags are case-sensitive decoded keys. Values are ignored: `?sample=false`
/// still enables sample startup, and repeated keys have the same presence semantics.
pub(crate) fn has_query_flag(query: &str, flag: &str) -> bool {
    let query = query.strip_prefix('?').unwrap_or(query);
    form_urlencoded::parse(query.as_bytes()).any(|(key, _)| key == flag)
}

/// Missa: the document named by `?open=`, when it is a same-origin path to a file the editor
/// opens (`/organization/x/issues/3/design/file.idml`). Anything else, including another host
/// (`https://…`, `//host/…`, `/\\host/…`) or a page without a document extension, is refused, so
/// a link can never make the editor fetch from somewhere the signed-in page doesn't own.
/// Returns the path to fetch and the file name to open it under.
pub(crate) fn open_path(query: &str) -> Option<(String, String)> {
    let query = query.strip_prefix('?').unwrap_or(query);
    let (_, path) = form_urlencoded::parse(query.as_bytes()).find(|(key, _)| key == "open")?;
    let path = path.into_owned();
    if !path.starts_with('/') || path.starts_with("//") || path.contains('\\') || path.chars().any(char::is_control) {
        return None;
    }
    let file_part = path.split(['?', '#']).next()?;
    let raw = file_part.rsplit('/').next().filter(|n| !n.is_empty())?;
    let name = percent_decode(raw);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    matches!(ext.as_str(), "designcraft" | "idml").then_some((path, name))
}

/// Missa: where Save sends the document opened from `open` (a path `open_path` accepted): the
/// same folder and name, as a `.designcraft` file. Missa keeps each save as a new version.
pub(crate) fn save_path(open: &str) -> Option<String> {
    let file_part = open.split(['?', '#']).next()?;
    let (folder, file) = file_part.rsplit_once('/')?;
    let (stem, _) = file.rsplit_once('.')?;
    (!stem.is_empty()).then(|| format!("{folder}/{stem}.designcraft"))
}

/// `%XX` escapes decoded, as a link spells a file name (`Issue%2022.idml`). Anything that isn't a
/// valid escape or valid UTF-8 is kept as written.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(Some(h)), Some(Some(l))) = (bytes.get(i + 1).map(|b| hex(*b)), bytes.get(i + 2).map(|b| hex(*b)))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

pub(crate) fn mime_for(name: &str) -> &'static str {
    match name.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("pdf") => "application/pdf",
        Some("designcraft") => designcraft_format::MIME,
        Some("idml") => "application/vnd.adobe.indesign-idml-package",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::{has_query_flag, mime_for, open_path, save_path};

    #[test]
    fn open_takes_only_same_origin_document_paths() {
        let (path, name) = open_path("?open=%2Forganization%2Flolwe%2Fissues%2F3%2Fdesign%2Fissue-3.idml").unwrap_or_default();
        assert!(path.starts_with("/organization/") && name == "issue-3.idml", "{path} {name}");
        assert!(open_path("?open=/files/a.designcraft%3Fv%3D2").is_some_and(|(_, n)| n == "a.designcraft"));
        for refused in [
            "",
            "?sample",
            "?open=https://evil.example/a.idml",
            "?open=//evil.example/a.idml",
            "?open=/%5Cevil.example/a.idml",
            "?open=/files/page.html",
            "?open=/files/",
            "?open=files/a.idml",
            "?open=/files/a.idml%0A",
        ] {
            assert!(open_path(refused).is_none(), "{refused}");
        }
    }

    #[test]
    fn saves_go_beside_the_opened_file_and_names_read_as_written() {
        let (path, name) = open_path("?open=%2Fapi%2Forgs%2Fo%2Fissues%2Fi%2Fdesign%2FIssue%252022.idml").unwrap_or_default();
        assert_eq!(name, "Issue 22.idml");
        assert_eq!(save_path(&path).as_deref(), Some("/api/orgs/o/issues/i/design/Issue%2022.designcraft"));
        assert_eq!(save_path("/a/b.designcraft?v=2").as_deref(), Some("/a/b.designcraft"));
        assert_eq!(save_path("/a/.idml"), None);
    }

    #[test]
    fn startup_flags_match_complete_decoded_keys() {
        for flag in ["sample", "webgl"] {
            for query in [format!("?{flag}"), format!("?{flag}=false"), format!("?{flag}=1&{flag}=0"), format!("?other=value&{flag}")] {
                assert!(has_query_flag(&query, flag), "{query}");
            }
            for query in [
                String::new(),
                format!("?note={flag}"),
                format!("?{flag}d=1"),
                format!("?note=not_a_{flag}"),
                "?note=%73ample%77ebgl".to_string(),
                format!("?{flag}+extra=1"),
                format!("?{flag}%26extra=1"),
                format!("?{flag}%3Dextra=1"),
            ] {
                assert!(!has_query_flag(&query, flag), "{query}");
            }
        }
        assert!(has_query_flag("?webgl&sample", "sample"));
        assert!(has_query_flag("?webgl&sample", "webgl"));
        assert!(has_query_flag("?%73ample", "sample"));
        assert!(has_query_flag("?%77ebgl", "webgl"));
        assert!(!has_query_flag("?Sample&WEBGL", "sample"));
        assert!(!has_query_flag("?Sample&WEBGL", "webgl"));
    }

    #[test]
    fn download_mime_matches_native_package_and_existing_exports() {
        for (name, expected) in [
            ("Quarterly.designcraft", designcraft_format::MIME),
            ("日本語.multi.DEsignCraft", designcraft_format::MIME),
            ("page.pdf", "application/pdf"),
            ("page.png", "image/png"),
            ("page.jpg", "image/jpeg"),
            ("page.JPEG", "image/jpeg"),
            ("document.idml", "application/vnd.adobe.indesign-idml-package"),
            ("unknown.bin", "application/octet-stream"),
        ] {
            assert_eq!(mime_for(name), expected, "{name}");
        }
    }
}

/// Read a drop in order, publishing nothing unless every file can be read.
pub(crate) async fn read_batch<T, R, E, F, Fut>(files: impl IntoIterator<Item = T>, mut read: F) -> Result<Vec<R>, E>
where
    F: FnMut(T) -> Fut,
    Fut: std::future::Future<Output = Result<R, E>>,
{
    let mut result = Vec::new();
    for file in files {
        result.push(read(file).await?);
    }
    Ok(result)
}

#[cfg(test)]
mod batch_tests {
    #[test]
    fn failed_read_aborts_the_whole_batch() {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let mut seen = Vec::new();
        let result = {
            let future = super::read_batch([1, 2, 3], |n| {
                seen.push(n);
                std::future::ready(if n == 2 { Err("read failed") } else { Ok(n) })
            });
            let mut future = std::pin::pin!(future);
            future.as_mut().poll(&mut Context::from_waker(Waker::noop()))
        };
        assert_eq!(result, Poll::Ready(Err("read failed")));
        assert_eq!(seen, [1, 2]);
        let mut success = std::pin::pin!(super::read_batch([3, 1], |n| std::future::ready(Ok::<_, ()>(n))));
        assert_eq!(success.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(Ok(vec![3, 1])));
    }
}
