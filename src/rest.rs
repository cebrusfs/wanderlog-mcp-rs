//! Client for the private REST API behind the Wanderlog web app.
//!
//! Endpoints are verified against the official web app (see docs/protocol.md). Request
//! URLs can contain trip keys, which are bearer secrets, so errors are built without URLs.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use reqwest::header::{ACCEPT, COOKIE, HeaderMap, HeaderValue, ORIGIN, SET_COOKIE};
use reqwest::{RequestBuilder, StatusCode};
use serde_json::{Value, json};

pub const BASE: &str = "https://wanderlog.com";

#[derive(Clone)]
pub struct Rest {
    http: reqwest::Client,
}

/// A trip the account can open, from the home listing.
#[derive(Clone)]
pub struct TripSummary {
    pub id: u64,
    /// Bearer capability for the trip: never show it to models or write it to logs.
    pub key: String,
    /// Whether `key` is an edit key (writes allowed).
    pub editable: bool,
    pub title: String,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub place_count: u64,
    pub edited_at: Option<String>,
    /// "own", "shared with you" or "friend".
    pub relation: &'static str,
}

impl std::fmt::Debug for TripSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TripSummary")
            .field("id", &self.id)
            .field("key", &"<redacted>")
            .field("editable", &self.editable)
            .field("title", &self.title)
            .field("relation", &self.relation)
            .finish_non_exhaustive()
    }
}

impl Rest {
    pub fn new(cookie: &str) -> Result<Self> {
        Ok(Self {
            http: Self::client(Some(cookie))?,
        })
    }

    fn client(cookie: Option<&str>) -> Result<reqwest::Client> {
        let mut headers = HeaderMap::new();
        if let Some(cookie) = cookie {
            let mut session = HeaderValue::from_str(&format!("connect.sid={cookie}"))
                .context("session cookie has invalid characters")?;
            session.set_sensitive(true);
            headers.insert(COOKIE, session);
        }
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(ORIGIN, HeaderValue::from_static(BASE));
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(crate::USER_AGENT)
            .timeout(Duration::from_secs(30))
            // URLs carry trip keys: never leak them in a Referer, and never follow redirects
            // (the API does not use them, and a cross-host hop would get the key).
            .referer(false)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(http)
    }

    /// Exchange a Wanderlog email and password for a verified session, without persisting either.
    pub async fn login(email: &str, password: &str) -> Result<(String, Value)> {
        Self::login_at(BASE, email, password).await
    }

    async fn login_at(base: &str, email: &str, password: &str) -> Result<(String, Value)> {
        let email = email.trim();
        ensure!(!email.is_empty(), "Wanderlog email cannot be empty");
        ensure!(!password.is_empty(), "Wanderlog password cannot be empty");

        let http = Self::client(None)?;
        let (headers, _) = login_response(
            http.post(format!("{base}/api/user/login")).json(&json!({
                "email": email,
                "password": password,
                "platform": "web",
            })),
            "log in",
        )
        .await?;
        let raw = headers
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|header| header.to_str().ok())
            .rfind(|value| value.starts_with("connect.sid="))
            .context("Wanderlog did not return a connect.sid session cookie")?;
        let cookie =
            crate::auth::normalize(raw).context("Wanderlog returned an invalid session cookie")?;

        // A successful response (or an anonymous cookie) alone does not prove authentication.
        // Verify using only the new cookie before the caller can replace the stored session.
        let (_, body) = login_response(
            Self::client(Some(&cookie))?.get(format!("{base}/api/user")),
            "verify login",
        )
        .await?;
        let user = body
            .get("user")
            .filter(|user| user.get("id").and_then(Value::as_u64).is_some())
            .context("the new Wanderlog session is not logged in")?;
        Ok((cookie, user.clone()))
    }

    async fn call(&self, request: RequestBuilder, what: &str) -> Result<Value> {
        let response = request
            .send()
            .await
            .map_err(|e| anyhow!("{what}: {}", e.without_url()))?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            bail!(
                "{what}: not authorised (HTTP {status}); the Wanderlog session may have expired — run `wanderlog-mcp auth login` or supply a cookie with `auth set`"
            );
        }
        let body: Value = response.json().await.map_err(|e| {
            anyhow!(
                "{what}: unreadable response (HTTP {status}): {}",
                e.without_url()
            )
        })?;
        if !status.is_success() || body.get("success") == Some(&Value::Bool(false)) {
            let message = body
                .pointer("/messages/0")
                .or_else(|| body.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            bail!(
                "{what}: HTTP {status}: {}",
                crate::render::quote(message, Default::default())
            );
        }
        Ok(body)
    }

    async fn get(&self, path: &str, query: &[(&str, &str)], what: &str) -> Result<Value> {
        self.call(self.http.get(format!("{BASE}{path}")).query(query), what)
            .await
    }

    async fn post(&self, path: &str, body: &Value, what: &str) -> Result<Value> {
        self.call(self.http.post(format!("{BASE}{path}")).json(body), what)
            .await
    }

    /// The logged-in user, or `None` when the cookie is not (or no longer) a logged-in session.
    pub async fn current_user(&self) -> Result<Option<Value>> {
        let body = self.get("/api/user", &[], "check session").await?;
        Ok(body.get("user").filter(|u| !u.is_null()).cloned())
    }

    /// Trips visible on the home page: own, privately shared with the user, and friends'.
    pub async fn trips(&self) -> Result<Vec<TripSummary>> {
        let body = self.get("/api/tripPlans/home", &[], "list trips").await?;
        let mut trips: Vec<TripSummary> = Vec::new();
        let groups = [
            ("ownTripPlans", "own"),
            ("friendsPrivateSharedTripPlans", "shared with you"),
            ("friendsTripPlans", "friend"),
        ];
        for (field, relation) in groups {
            for t in body
                .get(field)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let (Some(id), Some(key)) = (
                    t.get("id").and_then(Value::as_u64),
                    t.get("key").and_then(Value::as_str),
                ) else {
                    continue;
                };
                if trips.iter().any(|known| known.id == id) {
                    continue;
                }
                let text = |k: &str| t.get(k).and_then(Value::as_str).map(str::to_owned);
                trips.push(TripSummary {
                    id,
                    key: key.to_owned(),
                    editable: t.get("keyType").and_then(Value::as_str) == Some("edit"),
                    title: text("title").unwrap_or_default(),
                    start_date: text("startDate"),
                    end_date: text("endDate"),
                    place_count: t.get("placeCount").and_then(Value::as_u64).unwrap_or(0),
                    edited_at: text("editedAt"),
                    relation,
                });
            }
        }
        Ok(trips)
    }

    /// Full trip payload: `tripPlan` (same document ShareDB serves) plus `resources` (geo, ...).
    pub async fn trip(&self, key: &str) -> Result<Value> {
        // Without clientSchemaVersion=2 the server answers "app version too old".
        self.get(
            &format!("/api/tripPlans/{key}"),
            &[("clientSchemaVersion", "2")],
            "load trip",
        )
        .await
    }

    /// Google-backed place autocomplete; `near` = (latitude, longitude, radius in metres).
    pub async fn autocomplete(
        &self,
        input: &str,
        near: Option<(f64, f64, f64)>,
    ) -> Result<Vec<Value>> {
        // location and radius are both required; they only bias results, so without a trip a
        // small circle at 0,0 acts as "no preference" (verified to still find places worldwide).
        let (latitude, longitude, radius) = near.unwrap_or((0.0, 0.0, 50_000.0));
        let request = json!({
            "input": input, "sessiontoken": uuid_v4(), "language": "en",
            "location": {"longitude": longitude, "latitude": latitude}, "radius": radius,
        })
        .to_string();
        let body = self
            .get(
                "/api/placesAPI/autocomplete/v2",
                &[("request", &request)],
                "search places",
            )
            .await?;
        // The endpoint mixes in free-text "search" suggestions without a place_id.
        Ok(body
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|p| {
                p.get("place_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty())
            })
            .cloned()
            .collect())
    }

    /// Google-style place details; the web client stores this object verbatim as `block.place`.
    pub async fn place_details(&self, place_id: &str) -> Result<Value> {
        let query = [("placeId", place_id), ("language", "en")];
        let body = self
            .get("/api/placesAPI/getPlaceDetails/v2", &query, "place details")
            .await?;
        body.get("data")
            .filter(|d| d.is_object())
            .cloned()
            .ok_or_else(|| anyhow!("place details: nothing found for {place_id}"))
    }

    /// Wanderlog image keys for a place (stored as `block.imageKeys`; apps show thumbnails from them).
    pub async fn place_photos(&self, place: &Value) -> Result<Vec<String>> {
        let id = place
            .get("place_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("place without place_id"))?;
        let body = self
            .post(
                &format!("/api/placePhotos/{}", encode_segment(id)),
                &json!({"place": place}),
                "place photos",
            )
            .await?;
        Ok(body
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect())
    }

    /// Wanderlog's own place metadata (description, typical visit duration, categories).
    pub async fn place_metadata(&self, place_id: &str, trip_key: &str) -> Result<Option<Value>> {
        let query = [
            ("placeIds", place_id),
            ("listId", trip_key),
            ("listType", "tripPlan"),
            ("ensurePlaceDetailsAreFresh", "true"),
            ("includeNeedsBooking", "true"),
        ];
        let body = self
            .get("/api/places/metadata", &query, "place metadata")
            .await?;
        Ok(body.pointer("/data/0").cloned())
    }

    /// Destinations (cities, regions, countries) matching `query`.
    pub async fn geo_search(&self, query: &str) -> Result<Vec<Value>> {
        let body = self
            .get(
                &format!("/api/geo/autocomplete/{}", encode_segment(query)),
                &[],
                "search destinations",
            )
            .await?;
        Ok(body
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Create a trip the way the web app's "Plan a new trip" form does (no dates; set them later).
    pub async fn create_trip(&self, geo_id: u64, privacy: &str) -> Result<Value> {
        let body = json!({
            "geoIds": [geo_id], "initialMapsPlaceIds": [], "initialSections": null, "initialEmailId": null,
            "type": "plan", "startDate": null, "endDate": null, "privacy": privacy, "isMapEmbed": false,
            "title": null, "autogenerateItineraryOptions": null, "language": "en",
        });
        let response = self.post("/api/tripPlans", &body, "create trip").await?;
        response
            .get("data")
            .cloned()
            .ok_or_else(|| anyhow!("create trip: no data returned"))
    }
}

/// Login responses may echo credentials; never include their bodies or server messages in errors.
async fn login_response(request: RequestBuilder, what: &str) -> Result<(HeaderMap, Value)> {
    let response = request
        .send()
        .await
        .map_err(|e| anyhow!("{what}: {}", e.without_url()))?;
    let status = response.status();
    if status == StatusCode::TOO_MANY_REQUESTS {
        bail!("{what}: Wanderlog rate limited the request (HTTP {status}); wait before retrying");
    }
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        bail!(
            "{what}: Wanderlog rejected the login (HTTP {status}); check your email and password"
        );
    }
    ensure!(
        status.is_success(),
        "{what}: Wanderlog returned HTTP {status}"
    );
    let headers = response.headers().clone();
    let body: Value = response
        .json()
        .await
        .map_err(|_| anyhow!("{what}: Wanderlog returned an unreadable response"))?;
    ensure!(
        body.get("success") == Some(&Value::Bool(true)),
        "{what}: Wanderlog did not accept the login; check your email and password"
    );
    Ok((headers, body))
}

/// Percent-encode one URL path segment (RFC 3986 unreserved characters pass through).
fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Random RFC 4122 v4 UUID (the autocomplete session token format the web client uses).
fn uuid_v4() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn response(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        )
    }

    async fn login_server(
        responses: Vec<String>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(5), async move {
                let mut requests = Vec::new();
                for response in responses {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    loop {
                        let mut buf = [0; 1024];
                        let n = stream.read(&mut buf).await.unwrap();
                        assert!(n > 0, "client closed before completing the request");
                        request.extend_from_slice(&buf[..n]);
                        let Some(end) = request.windows(4).position(|b| b == b"\r\n\r\n") else {
                            continue;
                        };
                        let headers = std::str::from_utf8(&request[..end]).unwrap();
                        let length = headers
                            .lines()
                            .filter_map(|line| line.split_once(':'))
                            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                    requests.push(String::from_utf8(request).unwrap());
                    stream.write_all(response.as_bytes()).await.unwrap();
                }
                requests
            })
            .await
            .expect("login requests exceeded the test deadline")
        });
        (base, server)
    }

    #[tokio::test]
    async fn login_verifies_the_new_cookie_without_forwarding_other_cookies() {
        let (base, server) = login_server(vec![
            response(
                "200 OK",
                "Set-Cookie: analytics=other; Path=/\r\nSet-Cookie: connect.sid=s%3Anew.session%3D; Path=/; HttpOnly; Secure\r\n",
                r#"{"success":true,"user":{"id":1,"username":"unverified"}}"#,
            ),
            response("200 OK", "", r#"{"success":true,"user":{"id":2,"username":"verified"}}"#),
        ])
        .await;
        let (cookie, user) = Rest::login_at(&base, " person@example.com ", " password ")
            .await
            .unwrap();
        assert_eq!(cookie, "s%3Anew.session%3D");
        assert_eq!(user["username"], "verified");
        let requests = server.await.unwrap();
        let (headers, body) = requests[0].split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with("POST /api/user/login HTTP/1.1\r\n"));
        assert!(!headers.to_ascii_lowercase().contains("\r\ncookie:"));
        assert_eq!(
            serde_json::from_str::<Value>(body).unwrap(),
            json!({"email":"person@example.com", "password":" password ", "platform":"web"})
        );
        assert!(requests[1].starts_with("GET /api/user HTTP/1.1\r\n"));
        assert!(requests[1].contains("connect.sid=s%3Anew.session%3D\r\n"));
        for private in ["analytics", "person@example.com", "password"] {
            assert!(!requests[1].contains(private));
        }
    }

    #[tokio::test]
    async fn login_failures_never_echo_credentials_or_server_messages() {
        let sensitive_body = r#"{"success":false,"messages":["person@example.com secret-password s%3Asecret.cookie"]}"#;
        for (status, body, expected) in [
            ("401 Unauthorized", sensitive_body, "rejected"),
            ("403 Forbidden", sensitive_body, "rejected"),
            ("429 Too Many Requests", sensitive_body, "rate limited"),
            ("500 Internal Server Error", sensitive_body, "HTTP 500"),
            ("302 Found", sensitive_body, "HTTP 302"),
            ("200 OK", sensitive_body, "did not accept"),
            ("200 OK", "secret-password", "unreadable response"),
        ] {
            let (base, server) = login_server(vec![response(
                status,
                "Set-Cookie: connect.sid=s%3Asecret.cookie; Path=/\r\nLocation: /must-not-follow\r\n",
                body,
            )])
            .await;
            let err = Rest::login_at(&base, "person@example.com", "secret-password")
                .await
                .unwrap_err();
            let message = format!("{err:#}");
            assert!(message.contains(expected), "{message}");
            for secret in ["person@example.com", "secret-password", "s%3Asecret.cookie"] {
                assert!(!message.contains(secret), "{message}");
            }
            assert_eq!(server.await.unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn login_requires_a_session_cookie_and_authenticated_user() {
        for headers in [
            "",
            "Set-Cookie: analytics=connect.sid=unrelated\r\n",
            "Set-Cookie: connect.sid=; Path=/\r\n",
            "Set-Cookie: connect.sid=invalid value; Path=/\r\n",
        ] {
            let (base, server) = login_server(vec![response(
                "200 OK",
                headers,
                r#"{"success":true,"user":{"id":1}}"#,
            )])
            .await;
            let err = Rest::login_at(&base, "person@example.com", "password")
                .await
                .unwrap_err();
            assert!(err.to_string().contains("session cookie"));
            assert_eq!(server.await.unwrap().len(), 1);
        }
        for (status, body, expected) in [
            ("200 OK", r#"{"success":true,"user":null}"#, "not logged in"),
            ("200 OK", r#"{"success":true,"user":{}}"#, "not logged in"),
            (
                "401 Unauthorized",
                "secret-password s%3Anew.session",
                "rejected",
            ),
        ] {
            let (base, server) = login_server(vec![
                response(
                    "200 OK",
                    "Set-Cookie: connect.sid=s%3Anew.session; Path=/\r\n",
                    r#"{"success":true,"user":{"id":1}}"#,
                ),
                response(status, "", body),
            ])
            .await;
            let err = Rest::login_at(&base, "person@example.com", "secret-password")
                .await
                .unwrap_err();
            let message = format!("{err:#}");
            assert!(message.contains(expected), "{message}");
            assert!(!message.contains("secret-password"));
            assert!(!message.contains("s%3Anew.session"));
            assert_eq!(server.await.unwrap().len(), 2);
        }
    }

    #[tokio::test]
    async fn login_rejects_empty_credentials_before_network_access() {
        for (email, password) in [("  ", "password"), ("person@example.com", "")] {
            let err = Rest::login_at("invalid-url", email, password)
                .await
                .unwrap_err();
            assert!(err.to_string().contains("cannot be empty"));
        }
    }

    #[test]
    fn debug_never_prints_the_trip_key() {
        let t = TripSummary {
            id: 1,
            key: "abcdefghijklmnop".into(),
            editable: true,
            title: "t".into(),
            start_date: None,
            end_date: None,
            place_count: 0,
            edited_at: None,
            relation: "own",
        };
        assert!(!format!("{t:?}").contains("abcdefghijklmnop"));
    }

    #[test]
    fn segments_and_uuids() {
        assert_eq!(encode_segment("ChIJ_x-1.~"), "ChIJ_x-1.~");
        assert_eq!(
            encode_segment("東京 tower/1"),
            "%E6%9D%B1%E4%BA%AC%20tower%2F1"
        );
        let id = uuid_v4();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
    }
}
