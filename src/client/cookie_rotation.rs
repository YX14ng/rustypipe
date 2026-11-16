//! Keeping a cookie session alive the way the YouTube website does.
//!
//! While a YouTube tab is open, Google keeps renewing the short-lived session cookies
//! (`__Secure-1PSIDTS`, `__Secure-3PSIDTS` and the `*SIDCC` cookies): the page embeds
//! `accounts.youtube.com/RotateCookiesPage`, which posts a token to
//! `accounts.youtube.com/RotateCookies` about every ten minutes and gets the new cookies back in
//! `Set-Cookie` headers. A session that is never renewed this way eventually stops working.
//!
//! Google accepts the current and the previous value of a rotated cookie, so a request still
//! using the old header right after a rotation works, but the new cookies must be kept: a
//! rotation whose answer is lost leaves only superseded cookies behind.

use std::time::Duration;

use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::{header, StatusCode};

use crate::error::{AuthError, Error, ExtractionError};

use super::RustyPipe;

const ROTATE_PAGE_URL: &str =
    "https://accounts.youtube.com/RotateCookiesPage?origin=https://www.youtube.com&yt_pid=1";
const ROTATE_URL: &str = "https://accounts.youtube.com/RotateCookies";
const ACCOUNTS_ORIGIN: &str = "https://accounts.youtube.com";

/// How long to wait until the next rotation when Google does not say.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(600);
/// Google answers 429 to rotations less than a minute apart.
const MIN_INTERVAL: Duration = Duration::from_secs(60);

impl RustyPipe {
    /// Renew the user authentication cookie, like an open YouTube tab does.
    ///
    /// Asks Google for new session cookies, merges them into the stored cookie and saves it.
    /// Returns how long to wait before the next rotation (about ten minutes); rotating more
    /// than once a minute is refused by Google.
    ///
    /// Returns [`AuthError::NoLogin`] when no cookie is set or Google no longer accepts the
    /// session (the user has to sign in again).
    ///
    /// If the cookie is replaced or removed while the rotation runs, the new cookie is kept and
    /// the rotated one is dropped.
    pub async fn user_auth_rotate_cookie(&self) -> Result<Duration, Error> {
        let rotated = self.user_auth_cookie()?.cookie;

        let page = self
            .inner
            .http
            .get(ROTATE_PAGE_URL)
            .header(header::COOKIE, &rotated)
            .build()?;
        let page = self.inner.http.execute(page).await?;
        check_status(page.status())?;
        let mut set_cookies = set_cookie_values(page.headers());
        let page = page.text().await?;
        let body = rotation_body(&page).ok_or(Error::Extraction(ExtractionError::InvalidData(
            "no rotation token on RotateCookiesPage".into(),
        )))?;

        let rotate = self
            .inner
            .http
            .post(ROTATE_URL)
            .header(header::COOKIE, &rotated)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, ACCOUNTS_ORIGIN)
            .header(header::REFERER, ROTATE_PAGE_URL)
            .body(body)
            .build()?;
        let rotate = self.inner.http.execute(rotate).await?;
        check_status(rotate.status())?;
        set_cookies.extend(set_cookie_values(rotate.headers()));
        let interval = next_interval(&rotate.text().await.unwrap_or_default());

        let renewed = merge_set_cookies(&rotated, &set_cookies, now_year());
        let changed = {
            let mut stored = self.inner.cache.auth_cookie.write().unwrap();
            match stored.as_mut() {
                Some(stored) if stored.cookie == rotated && stored.cookie != renewed => {
                    stored.cookie = renewed;
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.store_cache().await;
        }
        Ok(interval)
    }
}

/// Google answers 401 to a session it no longer accepts.
fn check_status(status: StatusCode) -> Result<(), Error> {
    if status == StatusCode::UNAUTHORIZED {
        Err(Error::Auth(AuthError::NoLogin))
    } else if status.is_client_error() || status.is_server_error() {
        Err(Error::HttpStatus(
            status.as_u16(),
            "cookie rotation failed".into(),
        ))
    } else {
        Ok(())
    }
}

fn set_cookie_values(headers: &header::HeaderMap) -> Vec<String> {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok().map(str::to_owned))
        .collect()
}

/// The body the rotation page posts, from its `init('<token>', m, o, l, interval)` call: the
/// protobuf-as-JSON array `[m, "<token>", o, l]`, zeros written as `null` and trailing nulls
/// left out (`[null,"-3614468034866697709",1]`).
fn rotation_body(page: &str) -> Option<String> {
    static INIT: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"init\('(-?\d+)'\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)").unwrap()
    });
    let caps = INIT.captures(page)?;
    let number = |i: usize| match caps[i].parse::<f64>() {
        Ok(n) if n != 0.0 => caps[i].to_owned(),
        _ => "null".to_owned(),
    };
    let mut fields = vec![number(2), format!("\"{}\"", &caps[1]), number(3), number(4)];
    while fields.last().map(String::as_str) == Some("null") {
        fields.pop();
    }
    Some(format!("[{}]", fields.join(",")))
}

/// The wait Google suggests in its answer (`)]}'\n[["identity.hfcr",600],...]`), at least a
/// minute.
fn next_interval(answer: &str) -> Duration {
    static INTERVAL: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#""identity\.hfcr"\s*,\s*(\d+)"#).unwrap());
    INTERVAL
        .captures(answer)
        .and_then(|caps| caps[1].parse().ok())
        .map_or(DEFAULT_INTERVAL, Duration::from_secs)
        .max(MIN_INTERVAL)
}

fn now_year() -> i32 {
    crate::util::now_sec().year()
}

/// The `cookie` header with the cookies of `set_cookies` (`Set-Cookie` header values) applied:
/// new values replace old ones, new cookies are added and expired ones removed.
///
/// Only cookies set for the whole `youtube.com` domain are taken, since only those are sent to
/// the YouTube sites; cookies just for `accounts.youtube.com` are left out.
fn merge_set_cookies(cookie: &str, set_cookies: &[String], this_year: i32) -> String {
    let mut cookies: Vec<(String, String)> = cookie
        .split(';')
        .filter_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            Some((name.to_owned(), value.to_owned()))
        })
        .collect();

    for set_cookie in set_cookies {
        let mut parts = set_cookie.split(';');
        let Some((name, value)) = parts.next().and_then(|pair| pair.trim().split_once('=')) else {
            continue;
        };
        let mut for_youtube = false;
        let mut expired = false;
        for attribute in parts {
            let (key, val) = attribute
                .trim()
                .split_once('=')
                .unwrap_or((attribute.trim(), ""));
            if key.eq_ignore_ascii_case("domain") {
                for_youtube = val
                    .trim_start_matches('.')
                    .eq_ignore_ascii_case("youtube.com");
            } else if key.eq_ignore_ascii_case("max-age") {
                expired |= matches!(val.parse::<i64>(), Ok(age) if age <= 0);
            } else if key.eq_ignore_ascii_case("expires") {
                expired |= matches!(expires_year(val), Some(year) if year < this_year);
            }
        }
        if !for_youtube {
            continue;
        }
        let existing = cookies.iter().position(|(old, _)| old == name);
        match (existing, expired) {
            (Some(i), true) => {
                cookies.remove(i);
            }
            (Some(i), false) => value.clone_into(&mut cookies[i].1),
            (None, false) => cookies.push((name.to_owned(), value.to_owned())),
            (None, true) => {}
        }
    }

    cookies
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The year of an `Expires` date like "Mon, 01-Jan-1990 00:00:00 GMT"; Google deletes cookies
/// by setting a date long past.
fn expires_year(date: &str) -> Option<i32> {
    static YEAR: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b(\d{4})\b").unwrap());
    YEAR.captures(date)?[1].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_body_from_the_page() {
        let page = "<script nonce=\"x\">init('-3614468034866697709', 0, 1, 0, 600);</script>";
        assert_eq!(
            rotation_body(page).as_deref(),
            Some(r#"[null,"-3614468034866697709",1]"#)
        );
        let page = "init('42',3,0,7)";
        assert_eq!(rotation_body(page).as_deref(), Some(r#"[3,"42",null,7]"#));
        assert_eq!(rotation_body("<html>signed out</html>"), None);
    }

    #[test]
    fn reads_the_suggested_interval() {
        let answer = ")]}'\n[[\"identity.hfcr\",600],[\"di\",35]]";
        assert_eq!(next_interval(answer), Duration::from_secs(600));
        assert_eq!(next_interval(")]}'\n[[\"identity.hfcr\",5]]"), MIN_INTERVAL);
        assert_eq!(next_interval(""), DEFAULT_INTERVAL);
    }

    #[test]
    fn merges_the_rotated_cookies() {
        let cookie = "SID=sid; __Secure-1PSIDTS=old; SIDCC=cc; HSID=h";
        let set_cookies = [
            "__Secure-1PSIDTS=new; Domain=.youtube.com; Path=/; Secure; HttpOnly; \
             Expires=Wed, 15-Nov-2028 00:00:00 GMT"
                .to_owned(),
            "__Secure-3PSIDTS=three; Domain=.youtube.com; Path=/; Secure; SameSite=none".to_owned(),
            "SIDCC=; Domain=.youtube.com; Expires=Mon, 01-Jan-1990 00:00:00 GMT".to_owned(),
            "HSID=gone; Domain=youtube.com; Max-Age=0".to_owned(),
            "ACCOUNT_CHOOSER=x; Path=/; Secure".to_owned(),
            "SIDCC=elsewhere; Domain=.google.com".to_owned(),
        ];
        assert_eq!(
            merge_set_cookies(cookie, &set_cookies, 2026),
            "SID=sid; __Secure-1PSIDTS=new; __Secure-3PSIDTS=three"
        );
    }

    #[test]
    fn keeps_the_cookie_when_nothing_rotated() {
        let cookie = "SID=sid; __Secure-1PSIDTS=old";
        assert_eq!(merge_set_cookies(cookie, &[], 2026), cookie);
    }
}
