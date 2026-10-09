//! Client for the private REST API behind the Wanderlog web app.
//!
//! Endpoints are verified against the official web app (see docs/protocol.md). Request
//! URLs can contain trip keys, which are bearer secrets, so errors are built without URLs.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::clock::{Clock, SystemClock};
use crate::errors::{AuthenticationFailed, OutcomeUnknown};
use anyhow::{Context, Result, anyhow, bail, ensure};
use reqwest::header::{ACCEPT, COOKIE, HeaderMap, HeaderValue, ORIGIN, SET_COOKIE};
use reqwest::{RequestBuilder, StatusCode};
use serde_json::{Value, json};

pub const BASE: &str = "https://wanderlog.com";

#[derive(Clone)]
pub struct Rest {
    http: reqwest::Client,
    cooldown: Arc<Mutex<Option<Cooldown>>>,
    base: String,
    cookie: String,
    clock: Arc<dyn Clock>,
}

#[derive(Clone, Copy)]
struct Cooldown {
    started: Instant,
    seconds: u64,
    retry_after_seconds: Option<u64>,
}

/// A local cooldown or an HTTP 429; no request is automatically replayed.
#[derive(Debug)]
pub struct RateLimited {
    pub retry_after_seconds: Option<u64>,
    pub retry_in_seconds: u64,
}

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Wanderlog rate limited the request (HTTP 429); retry in {} seconds",
            self.retry_in_seconds
        )
    }
}

impl std::error::Error for RateLimited {}

fn rate_limit_at(headers: &HeaderMap, now: SystemTime) -> RateLimited {
    let retry_after_seconds = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|header| header.to_str().ok())
        .and_then(|value| {
            let value = value.trim();
            if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
                Some(value.parse::<u64>().unwrap_or(u64::MAX))
            } else {
                httpdate::parse_http_date(value).ok().map(|date| {
                    date.duration_since(now)
                        .map(|duration| {
                            duration
                                .as_secs()
                                .saturating_add(u64::from(duration.subsec_nanos() > 0))
                        })
                        .unwrap_or(0)
                })
            }
        });
    RateLimited {
        retry_after_seconds,
        retry_in_seconds: retry_after_seconds.unwrap_or(60),
    }
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
        Self::with_base_url(cookie, BASE)
    }
    /// Explicit endpoint injection. TLS is required except on literal loopback hosts.
    /// Custom origins receive the supplied credentials. Redirects are never followed.
    pub fn with_base_url(cookie: &str, base: &str) -> Result<Self> {
        let url = validate_endpoint(base, "https", "http")?;
        ensure!(
            url.path() == "/" && url.query().is_none(),
            "API base URL must be an origin"
        );
        let cookie = if cookie.is_empty() {
            String::new()
        } else {
            crate::session::normalize(cookie)?
        };
        Ok(Self {
            http: Self::client(
                (!cookie.is_empty()).then_some(cookie.as_str()),
                is_loopback(&url),
            )?,
            cooldown: Arc::new(Mutex::new(None)),
            base: url.as_str().trim_end_matches('/').to_owned(),
            cookie,
            clock: Arc::new(SystemClock),
        })
    }
    /// Clones share cooldown state. Configure the injected clock before making requests.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }
    #[cfg(test)]
    pub(crate) fn for_test(base: &str) -> Result<Self> {
        Self::with_base_url("", base)
    }
    fn base(&self) -> &str {
        &self.base
    }
    fn client(cookie: Option<&str>, local: bool) -> Result<reqwest::Client> {
        let mut headers = HeaderMap::new();
        if let Some(cookie) = cookie {
            let mut session = HeaderValue::from_str(&format!("connect.sid={cookie}"))
                .context("session cookie has invalid characters")?;
            session.set_sensitive(true);
            headers.insert(COOKIE, session);
        }
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(ORIGIN, HeaderValue::from_static(BASE));
        let mut builder = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(crate::USER_AGENT)
            .timeout(Duration::from_secs(30))
            // URLs carry trip keys: never leak them in a Referer, and never follow redirects
            // (the API does not use them, and a cross-host hop would get the key).
            .referer(false)
            .redirect(reqwest::redirect::Policy::none());
        if local {
            builder = builder.no_proxy();
        }
        Ok(builder.build()?)
    }

    /// Exchange a Wanderlog email and password for a verified session, without persisting either.
    pub async fn login(email: &str, password: &str) -> Result<(String, Value)> {
        Self::login_with_base_url(BASE, email, password).await
    }

    pub async fn login_with_base_url(
        base: &str,
        email: &str,
        password: &str,
    ) -> Result<(String, Value)> {
        let email = email.trim();
        ensure!(!email.is_empty(), "Wanderlog email cannot be empty");
        ensure!(!password.is_empty(), "Wanderlog password cannot be empty");

        let url = validate_endpoint(base, "https", "http")?;
        ensure!(
            url.path() == "/" && url.query().is_none(),
            "API base URL must be an origin"
        );
        let base = url.as_str().trim_end_matches('/');
        let http = Self::client(None, is_loopback(&url))?;
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
        let cookie = crate::session::normalize(raw)
            .context("Wanderlog returned an invalid session cookie")?;

        // A successful response (or an anonymous cookie) alone does not prove authentication.
        // Verify using only the new cookie before the caller can replace the stored session.
        let (_, body) = login_response(
            Self::client(Some(&cookie), is_loopback(&url))?.get(format!("{base}/api/user")),
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
        // All REST endpoints share this cooldown, including writes, so clones cannot
        // continue hammering the account after one endpoint reports a limit.
        {
            let mut cooldown = self.cooldown.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(active) = *cooldown {
                let elapsed = self
                    .clock
                    .now()
                    .saturating_duration_since(active.started)
                    .as_secs();
                if elapsed < active.seconds {
                    return Err(RateLimited {
                        retry_after_seconds: active.retry_after_seconds,
                        retry_in_seconds: active.seconds - elapsed,
                    }
                    .into());
                }
                *cooldown = None;
            }
        }
        let request = request
            .build()
            .map_err(|_| anyhow!("{what}: invalid request"))?;
        let write =
            request.method() == reqwest::Method::POST && request.url().path() == "/api/tripPlans";
        let mut secrets = vec![self.cookie.clone()];
        // Trip keys are 10–16 letters; shorter segments (such as `home`) are routes, not secrets.
        if let Some(key) = request.url().path().strip_prefix("/api/tripPlans/")
            && key.len() >= 8
        {
            secrets.push(key.to_owned());
        }
        for (name, value) in request.url().query_pairs() {
            if name == "listId" {
                secrets.push(value.into_owned());
            }
        }
        let classify = |message: String| {
            if write {
                anyhow::Error::new(OutcomeUnknown(message))
            } else {
                anyhow!("{message}")
            }
        };
        let response = self
            .http
            .execute(request)
            .await
            .map_err(|e| classify(redact(&format!("{what}: {}", e.without_url()), &secrets)))?;
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            let error = rate_limit_at(response.headers(), self.clock.system_time());
            let mut cooldown = self.cooldown.lock().unwrap_or_else(|e| e.into_inner());
            let now = self.clock.now();
            let next = Cooldown {
                started: now,
                seconds: error.retry_in_seconds,
                retry_after_seconds: error.retry_after_seconds,
            };
            // Concurrent responses may extend a cooldown but never shorten it.
            if cooldown.is_none_or(|active| {
                active
                    .seconds
                    .saturating_sub(now.saturating_duration_since(active.started).as_secs())
                    <= next.seconds
            }) {
                *cooldown = Some(next);
            }
            return Err(error.into());
        }
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(AuthenticationFailed {
                status: status.as_u16(),
            }
            .into());
        }
        let body: Value = response.json().await.map_err(|e| {
            let message = redact(
                &format!(
                    "{what}: unreadable response (HTTP {status}): {}",
                    e.without_url()
                ),
                &secrets,
            );
            if write && (status.is_success() || status.is_server_error()) {
                classify(message)
            } else {
                anyhow!("{message}")
            }
        })?;
        if !status.is_success() || body.get("success") == Some(&Value::Bool(false)) {
            let message = body
                .pointer("/messages/0")
                .or_else(|| body.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            let message = redact(
                &format!(
                    "{what}: HTTP {status}: {}",
                    crate::render::quote(message, Default::default())
                ),
                &secrets,
            );
            return Err(if write && status.is_server_error() {
                classify(message)
            } else {
                anyhow!("{message}")
            });
        }
        Ok(body)
    }

    async fn get(&self, path: &str, query: &[(&str, &str)], what: &str) -> Result<Value> {
        self.call(
            self.http.get(format!("{}{path}", self.base())).query(query),
            what,
        )
        .await
    }

    async fn post(&self, path: &str, body: &Value, what: &str) -> Result<Value> {
        self.call(
            self.http.post(format!("{}{path}", self.base())).json(body),
            what,
        )
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
            &format!("/api/tripPlans/{}", encode_segment(key)),
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

    /// Batch Google-style details; the caller owns chunking and partial results.
    pub async fn multiple_place_details(&self, ids: &[String]) -> Result<Vec<Value>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut query: Vec<(&str, &str)> =
            ids.iter().map(|id| ("placeIds[]", id.as_str())).collect();
        query.push(("language", "en"));
        let body = self
            .get(
                "/api/placesAPI/getMultiplePlaceDetails",
                &query,
                "multiple place details",
            )
            .await?;
        let data = body
            .get("data")
            .and_then(Value::as_array)
            .context("multiple place details: expected a data array")?;
        // A failed entry must not discard other successful places; the caller checks IDs.
        Ok(data
            .iter()
            .filter(|place| place.is_object())
            .cloned()
            .collect())
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
        let keys = body
            .get("data")
            .and_then(Value::as_array)
            .context("place photos: expected an image key array")?;
        keys.iter()
            .map(|key| {
                key.as_str()
                    .map(str::to_owned)
                    .context("place photos: expected string image keys")
            })
            .collect()
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
            .filter(|data| {
                data.get("id").and_then(Value::as_u64).is_some()
                    && data
                        .get("key")
                        .and_then(Value::as_str)
                        .is_some_and(|k| !k.is_empty())
            })
            .cloned()
            .ok_or_else(|| {
                anyhow::Error::new(OutcomeUnknown(
                    "create trip: incomplete confirmation".into(),
                ))
            })
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
        return Err(rate_limit_at(response.headers(), SystemTime::now()).into());
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
pub(crate) fn encode_segment(s: &str) -> String {
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

pub(crate) fn is_loopback(url: &reqwest::Url) -> bool {
    url.host_str()
        .and_then(|s| s.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
        .is_some_and(|ip| ip.is_loopback())
}
pub(crate) fn validate_endpoint(raw: &str, tls: &str, plain: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw).map_err(|_| anyhow!("invalid transport endpoint"))?;
    ensure!(
        url.username().is_empty() && url.password().is_none() && url.fragment().is_none(),
        "transport endpoint must not contain credentials or fragments"
    );
    ensure!(
        url.scheme() == tls || (url.scheme() == plain && is_loopback(&url)),
        "TLS required except for literal loopback mocks"
    );
    Ok(url)
}
pub(crate) fn redact(text: &str, secrets: &[String]) -> String {
    let mut result = text.to_owned();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        let decoded = percent_encoding::percent_decode_str(secret)
            .decode_utf8_lossy()
            .into_owned();
        for variant in [secret.clone(), encode_segment(secret), decoded] {
            if !variant.is_empty() {
                result = result.replace(&variant, "<redacted>");
            }
        }
    }
    result
}

#[cfg(test)]
#[path = "tests/rest.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "rest_tests.rs"]
mod coverage_tests;
