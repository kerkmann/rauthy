use crate::{Assets, ReqPrincipal};
use actix_web::http::header::{HeaderValue, CONTENT_TYPE};
use actix_web::http::{header, StatusCode};
use actix_web::web::Json;
use actix_web::{get, post, put, web, HttpRequest, HttpResponse, Responder};
use cryptr::EncKeys;
use rauthy_common::constants::{
    APPLICATION_JSON, CACHE_NAME_LOGIN_DELAY, HEADER_HTML, IDX_LOGIN_TIME, RAUTHY_VERSION,
};
use rauthy_common::error_response::ErrorResponse;
use rauthy_common::utils::real_ip_from_req;
use rauthy_models::app_state::AppState;
use rauthy_models::entity::api_keys::{AccessGroup, AccessRights};
use rauthy_models::entity::app_version::LatestAppVersion;
use rauthy_models::entity::auth_providers::AuthProviderTemplate;
use rauthy_models::entity::colors::ColorEntity;
use rauthy_models::entity::is_db_alive;
use rauthy_models::entity::password::{PasswordHashTimes, PasswordPolicy};
use rauthy_models::entity::pow::PowEntity;
use rauthy_models::entity::users::User;
use rauthy_models::events::event::Event;
use rauthy_models::i18n::account::I18nAccount;
use rauthy_models::i18n::authorize::I18nAuthorize;
use rauthy_models::i18n::email_confirm_change_html::I18nEmailConfirmChangeHtml;
use rauthy_models::i18n::error::I18nError;
use rauthy_models::i18n::index::I18nIndex;
use rauthy_models::i18n::logout::I18nLogout;
use rauthy_models::i18n::password_reset::I18nPasswordReset;
use rauthy_models::i18n::register::I18nRegister;
use rauthy_models::i18n::SsrJson;
use rauthy_models::language::Language;
use rauthy_models::request::{
    EncKeyMigrateRequest, I18nContent, I18nRequest, PasswordHashTimesRequest,
    PasswordPolicyRequest, WhoamiRequestParam, WhoamiRequestParams,
};
use rauthy_models::response::{
    AppVersionResponse, Argon2ParamsResponse, EncKeysResponse, HealthResponse, LoginTimeResponse,
    PasswordPolicyResponse,
};
use rauthy_models::templates::{
    AccountHtml, AdminApiKeysHtml, AdminAttributesHtml, AdminBlacklistHtml, AdminClientsHtml,
    AdminConfigHtml, AdminDocsHtml, AdminGroupsHtml, AdminHtml, AdminRolesHtml, AdminScopesHtml,
    AdminSessionsHtml, AdminUsersHtml, IndexHtml,
};
use rauthy_service::encryption;
use redhac::{cache_get, cache_get_from, cache_get_value, QuorumHealth, QuorumState};
use semver::Version;
use std::borrow::Cow;
use std::str::FromStr;
use tracing::error;

#[get("/")]
pub async fn get_index(
    data: web::Data<AppState>,
    req: HttpRequest,
) -> Result<HttpResponse, ErrorResponse> {
    let colors = ColorEntity::find_rauthy(&data).await?;
    let lang = Language::try_from(&req).unwrap_or_default();
    let body = IndexHtml::build(&colors, &lang);

    Ok(HttpResponse::Ok().insert_header(HEADER_HTML).body(body))
}

#[get("/{_:.*}")]
pub async fn get_static_assets(
    path: web::Path<String>,
    accept_encoding: web::Header<header::AcceptEncoding>,
) -> HttpResponse {
    let path = path.into_inner();
    let accept_encoding = accept_encoding.into_inner();

    let mime = mime_guess::from_path(&path);
    let (p, encoding) = if accept_encoding.contains(&"br".parse().unwrap()) {
        (Cow::from(format!("{}.br", path)), "br")
    } else if accept_encoding.contains(&"gzip".parse().unwrap()) {
        (Cow::from(format!("{}.gz", path)), "gzip")
    } else {
        (Cow::from(path), "none")
    };

    match Assets::get(p.as_ref()) {
        Some(content) => HttpResponse::Ok()
            .insert_header(("cache-control", "max-age=2592000"))
            .insert_header(("content-encoding", encoding))
            .content_type(mime.first_or_octet_stream().as_ref())
            .body(content.data.into_owned()),
        None => {
            // Since this may resolve to a sub url path of any length, we cannot now, which
            // error template we need to serve -> just return not found
            HttpResponse::NotFound().finish()
        }
    }
}

#[post("/i18n")]
pub async fn post_i18n(
    req: HttpRequest,
    req_data: Json<I18nRequest>,
) -> Result<HttpResponse, ErrorResponse> {
    let lang = Language::try_from(&req).unwrap_or_default();
    let body = match req_data.content {
        I18nContent::Authorize => I18nAuthorize::build(&lang).as_json(),
        I18nContent::Account => I18nAccount::build(&lang).as_json(),
        I18nContent::EmailChangeConfirm => I18nEmailConfirmChangeHtml::build(&lang).as_json(),
        // Just return some default values for local dev -> dynamically built during prod
        I18nContent::Error => {
            I18nError::build_with(&lang, StatusCode::NOT_FOUND, Some("<empty>".to_string()))
                .as_json()
        }
        I18nContent::Index => I18nIndex::build(&lang).as_json(),
        I18nContent::Logout => I18nLogout::build(&lang).as_json(),
        I18nContent::PasswordReset => I18nPasswordReset::build(&lang).as_json(),
        I18nContent::Register => I18nRegister::build(&lang).as_json(),
    };

    Ok(HttpResponse::Ok()
        .insert_header((CONTENT_TYPE, APPLICATION_JSON))
        .body(body))
}

#[get("/account")]
pub async fn get_account_html(
    data: web::Data<AppState>,
    req: HttpRequest,
) -> Result<HttpResponse, ErrorResponse> {
    let colors = ColorEntity::find_rauthy(&data).await?;
    let lang = Language::try_from(&req).unwrap_or_default();
    let providers = AuthProviderTemplate::get_all_json_template(&data).await?;
    let body = AccountHtml::build(&colors, &lang, providers);

    Ok(HttpResponse::Ok().insert_header(HEADER_HTML).body(body))
}

#[get("/admin")]
pub async fn get_admin_html(data: web::Data<AppState>) -> Result<HttpResponse, ErrorResponse> {
    let colors = ColorEntity::find_rauthy(&data).await?;
    let body = AdminHtml::build(&colors);

    Ok(HttpResponse::Ok().insert_header(HEADER_HTML).body(body))
}

#[get("/admin/api_keys")]
pub async fn get_admin_api_keys_html(
    data: web::Data<AppState>,
) -> Result<HttpResponse, ErrorResponse> {
    let colors = ColorEntity::find_rauthy(&data).await?;
    let body = AdminApiKeysHtml::build(&colors);

    Ok(HttpResponse::Ok().insert_header(HEADER_HTML).body(body))
}

#[get("/admin/attributes")]
pub async fn get_admin_attr_html(data: web::Data<AppState>) -> Result<HttpResponse, ErrorResponse> {
    let colors = ColorEntity::find_rauthy(&data).await?;
    let body = AdminAttributesHtml::build(&colors);

    Ok(HttpResponse::Ok().insert_header(HEADER_HTML).body(body))
}

#[get("/admin/blacklist")]
pub async fn get_admin_blacklist_html(
    data: web::Data<AppState>,
) -> Result<HttpResponse, ErrorResponse> {
    let colors = ColorEntity::find_rauthy(&data).await?;
    let body = AdminBlacklistHtml::build(&colors);

    Ok(HttpResponse::Ok().insert_header(HEADER_HTML).body(body))
}

#[get("/admin/clients")]
pub async fn get_admin_clients_html(
    data: web::Data<AppState>,
) -> Result<HttpResponse, ErrorResponse> {
    let colors = ColorEntity::find_rauthy(&data).await?;
    let body = AdminClientsHtml::build(&colors);

    Ok(HttpResponse::Ok().insert_header(HEADER_HTML).body(body))
}

#[get("/admin/config")]
pub async fn get_admin_config_html(
    data: web::Data<AppState>,
) -> Result<HttpResponse, ErrorResponse> {
    let colors = ColorEntity::find_rauthy(&data).await?;
    let body = AdminConfigHtml::build(&colors);

    Ok(HttpResponse::Ok().insert_header(HEADER_HTML).body(body))
}

#[get("/admin/docs")]
pub async fn get_admin_docs_html(data: web::Data<AppState>) -> Result<HttpResponse, ErrorResponse> {
    let colors = ColorEntity::find_rauthy(&data).await?;
    let body = AdminDocsHtml::build(&colors);

    Ok(HttpResponse::Ok().insert_header(HEADER_HTML).body(body))
}

#[get("/admin/events")]
pub async fn get_admin_events_html(
    data: web::Data<AppState>,
) -> Result<HttpResponse, ErrorResponse> {
    let colors = ColorEntity::find_rauthy(&data).await?;
    let body = AdminUsersHtml::build(&colors);

    Ok(HttpResponse::Ok().insert_header(HEADER_HTML).body(body))
}

#[get("/admin/groups")]
pub async fn get_admin_groups_html(
    data: web::Data<AppState>,
) -> Result<HttpResponse, ErrorResponse> {
    let colors = ColorEntity::find_rauthy(&data).await?;
    let body = AdminGroupsHtml::build(&colors);

    Ok(HttpResponse::Ok().insert_header(HEADER_HTML).body(body))
}

#[get("/admin/roles")]
pub async fn get_admin_roles_html(
    data: web::Data<AppState>,
) -> Result<HttpResponse, ErrorResponse> {
    let colors = ColorEntity::find_rauthy(&data).await?;
    let body = AdminRolesHtml::build(&colors);

    Ok(HttpResponse::Ok().insert_header(HEADER_HTML).body(body))
}

#[get("/admin/scopes")]
pub async fn get_admin_scopes_html(
    data: web::Data<AppState>,
) -> Result<HttpResponse, ErrorResponse> {
    let colors = ColorEntity::find_rauthy(&data).await?;
    let body = AdminScopesHtml::build(&colors);

    Ok(HttpResponse::Ok().insert_header(HEADER_HTML).body(body))
}

#[get("/admin/sessions")]
pub async fn get_admin_sessions_html(
    data: web::Data<AppState>,
) -> Result<HttpResponse, ErrorResponse> {
    let colors = ColorEntity::find_rauthy(&data).await?;
    let body = AdminSessionsHtml::build(&colors);

    Ok(HttpResponse::Ok().insert_header(HEADER_HTML).body(body))
}

#[get("/admin/users")]
pub async fn get_admin_users_html(
    data: web::Data<AppState>,
) -> Result<HttpResponse, ErrorResponse> {
    let colors = ColorEntity::find_rauthy(&data).await?;
    let body = AdminUsersHtml::build(&colors);

    Ok(HttpResponse::Ok().insert_header(HEADER_HTML).body(body))
}

/// Check if the current session is valid
#[utoipa::path(
    get,
    path = "/auth_check",
    tag = "generic",
    responses(
        (status = 200, description = "Ok"),
        (status = 400, description = "BadRequest"),
        (status = 401, description = "Unauthorized"),
    ),
)]
#[get("/auth_check")]
pub async fn get_auth_check(principal: ReqPrincipal) -> Result<HttpResponse, ErrorResponse> {
    principal.validate_session_auth()?;
    Ok(HttpResponse::Ok().finish())
}

/// Check if access to the rauthy admin API is allowed
#[utoipa::path(
    get,
    path = "/auth_check_admin",
    tag = "generic",
    responses(
        (status = 200, description = "Ok"),
        (status = 400, description = "BadRequest"),
        (status = 401, description = "Unauthorized"),
    ),
)]
#[get("/auth_check_admin")]
pub async fn get_auth_check_admin(principal: ReqPrincipal) -> Result<HttpResponse, ErrorResponse> {
    principal.validate_admin_session()?;
    Ok(HttpResponse::Ok().finish())
}

/// Returns the existing encryption key ID's
///
/// Only the Key ID's are returned and never the key itself.
///
/// **Permissions**
/// - rauthy_admin
#[utoipa::path(
    get,
    path = "/encryption/keys",
    tag = "generic",
    responses(
        (status = 200, description = "Ok"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
)]
#[get("/encryption/keys")]
pub async fn get_enc_keys(principal: ReqPrincipal) -> Result<HttpResponse, ErrorResponse> {
    principal.validate_api_key_or_admin_session(AccessGroup::Secrets, AccessRights::Read)?;

    let enc_keys = EncKeys::get_static();
    let active = &enc_keys.enc_key_active;
    let keys = enc_keys
        .enc_keys
        .iter()
        .map(|(id, _)| id.as_str())
        .collect::<Vec<&str>>();

    let resp = EncKeysResponse { active, keys };
    Ok(HttpResponse::Ok().json(resp))
}

/// Migration of secrets to the new encryption key
///
/// **Permissions**
/// - rauthy_admin
#[utoipa::path(
    post,
    path = "/encryption/migrate",
    tag = "generic",
    responses(
        (status = 200, description = "Ok"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
)]
#[post("/encryption/migrate")]
pub async fn post_migrate_enc_key(
    data: web::Data<AppState>,
    req: HttpRequest,
    principal: ReqPrincipal,
    req_data: actix_web_validator::Json<EncKeyMigrateRequest>,
) -> Result<HttpResponse, ErrorResponse> {
    principal.validate_api_key_or_admin_session(AccessGroup::Secrets, AccessRights::Update)?;

    encryption::migrate_encryption_alg(&data, &req_data.key_id).await?;

    data.tx_events
        .send_async(Event::secrets_migrated(real_ip_from_req(&req)))
        .await
        .unwrap();

    Ok(HttpResponse::Ok().finish())
}

/// Returns the current Argon2ID parameters and the login time
///
/// The `login time` is the time it takes to complete a full login workflow incl password hashing.
///
/// **Permissions**
/// - rauthy_admin
#[utoipa::path(
    get,
    path = "/login_time",
    tag = "generic",
    responses(
        (status = 200, description = "LoginTimeResponse"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
)]
#[get("/login_time")]
pub async fn get_login_time(
    data: web::Data<AppState>,
    principal: ReqPrincipal,
) -> Result<HttpResponse, ErrorResponse> {
    principal.validate_api_key_or_admin_session(AccessGroup::Generic, AccessRights::Read)?;

    let login_time = cache_get!(
        u32,
        CACHE_NAME_LOGIN_DELAY.to_string(),
        IDX_LOGIN_TIME.to_string(),
        &data.caches.ha_cache_config,
        false
    )
    .await?
    .unwrap_or(2000);
    let argon2_params = Argon2ParamsResponse {
        m_cost: data.argon2_params.params.m_cost(),
        t_cost: data.argon2_params.params.t_cost(),
        p_cost: data.argon2_params.params.p_cost(),
    };
    let resp = LoginTimeResponse {
        argon2_params,
        login_time,
        num_cpus: num_cpus::get(),
    };
    Ok(HttpResponse::Ok().json(resp))
}

/// Find best the settings for argon2id
///
/// Does multiple argon2id computations to find the best settings for the given target values.
///
/// **Permissions**
/// - rauthy_admin
#[utoipa::path(
    post,
    path = "/password_hash_times",
    tag = "generic",
    request_body = PasswordHashTimesRequest,
    responses(
        (status = 200, description = "Ok", body = PasswordHashTimes),
        (status = 400, description = "BadRequest"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
)]
#[post("/password_hash_times")]
pub async fn post_password_hash_times(
    principal: ReqPrincipal,
    req_data: actix_web_validator::Json<PasswordHashTimesRequest>,
) -> Result<HttpResponse, ErrorResponse> {
    principal.validate_api_key_or_admin_session(AccessGroup::Generic, AccessRights::Create)?;

    PasswordHashTimes::compute(req_data.into_inner())
        .await
        .map(|r| HttpResponse::Ok().json(r))
}

/// Returns the currently configured password policy
///
/// **Permissions**
/// - authenticated
#[utoipa::path(
    get,
    path = "/password_policy",
    tag = "generic",
    responses(
        (status = 200, description = "Ok", body = PasswordPolicyResponse),
        (status = 401, description = "Unauthorized"),
    ),
)]
#[get("/password_policy")]
pub async fn get_password_policy(
    data: web::Data<AppState>,
    principal: ReqPrincipal,
) -> Result<HttpResponse, ErrorResponse> {
    principal.validate_session_auth()?;
    let rules = PasswordPolicy::find(&data).await?;
    Ok(HttpResponse::Ok().json(PasswordPolicyResponse::from(rules)))
}

/// Update the currently configured password policy
///
/// **Permissions**
/// - rauthy_admin
#[utoipa::path(
    put,
    path = "/password_policy",
    tag = "generic",
    request_body = PasswordPolicyRequest,
    responses(
        (status = 200, description = "Ok", body = PasswordPolicyResponse),
        (status = 400, description = "BadRequest"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
)]
#[put("/password_policy")]
pub async fn put_password_policy(
    data: web::Data<AppState>,
    principal: ReqPrincipal,
    req_data: actix_web_validator::Json<PasswordPolicyRequest>,
) -> Result<HttpResponse, ErrorResponse> {
    principal.validate_api_key_or_admin_session(AccessGroup::Secrets, AccessRights::Update)?;

    let mut rules = PasswordPolicy::find(&data).await?;
    rules.apply_req(req_data.into_inner());
    rules.save(&data).await?;
    Ok(HttpResponse::Ok().json(PasswordPolicyResponse::from(rules)))
}

/// Ping -> Pong
#[utoipa::path(
    get,
    path = "/ping",
    tag = "health",
    responses(
        (status = 200, description = "Ok"),
    ),
)]
#[get("/ping")]
pub async fn ping() -> impl Responder {
    HttpResponse::Ok().finish()
}

/// Request a Proof-of-Work Challenge
#[utoipa::path(
    get,
    path = "/pow",
    tag = "generic",
    responses(
        (status = 200, description = "Ok"),
    ),
)]
#[get("/pow")]
pub async fn get_pow(data: web::Data<AppState>) -> Result<HttpResponse, ErrorResponse> {
    // TODO request at least a session in init state or something like that?
    let pow = PowEntity::create(&data).await?;
    Ok(HttpResponse::Ok().body(pow.to_string()))
}

/// Updates the language for the logged in principal depending on the `locale` cookie
#[utoipa::path(
    post,
    path = "/update_language",
    tag = "generic",
    responses(
        (status = 200, description = "Valid Token / Session"),
        (status = 401, description = "Unauthorized"),
    ),
)]
#[post("/update_language")]
pub async fn post_update_language(
    data: web::Data<AppState>,
    principal: ReqPrincipal,
    req: HttpRequest,
) -> Result<HttpResponse, ErrorResponse> {
    principal.validate_session_auth()?;

    let user_id = principal.user_id()?;
    let mut user = User::find(&data, user_id.to_string()).await?;

    user.language = Language::try_from(&req).unwrap_or_default();
    user.update_language(&data).await?;

    Ok(HttpResponse::Ok().finish())
}

/// Backend health state
///
/// Health endpoint to get some additional information about the backend status, if it exists.
#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses(
        (status = 200, description = "Ok", body = HealthResponse),
    ),
)]
#[get("/health")]
pub async fn get_health(data: web::Data<AppState>) -> impl Responder {
    let is_db_alive = is_db_alive(&data.db).await;

    match data.caches.ha_cache_config.rx_health_state.borrow().clone() {
        None => {
            let body = HealthResponse {
                is_db_alive,
                cache_health: None,
                cache_state: None,
                cache_connected_hosts: None,
            };
            if is_db_alive {
                HttpResponse::Ok().json(body)
            } else {
                HttpResponse::InternalServerError().json(body)
            }
        }
        Some(hs) => {
            let is_bad = hs.health == QuorumHealth::Bad
                || hs.state == QuorumState::Undefined
                || hs.state == QuorumState::Retry;

            let body = HealthResponse {
                is_db_alive,
                cache_health: Some(hs.health),
                cache_state: Some(hs.state),
                cache_connected_hosts: Some(hs.connected_hosts),
            };

            if is_bad {
                HttpResponse::InternalServerError().json(body)
            } else {
                HttpResponse::Ok().json(body)
            }
        }
    }
}

/// Ready endpoint for kubernetes / docker health checks
#[utoipa::path(
    get,
    path = "/ready",
    tag = "health",
    responses(
        (status = 200, description = "Ok"),
        (status = 503, description = "ServiceUnavailable"),
    ),
)]
#[get("/ready")]
pub async fn get_ready(data: web::Data<AppState>) -> impl Responder {
    match data.caches.ha_cache_config.rx_health_state.borrow().clone() {
        None => {}
        Some(hs) => {
            if hs.health == redhac::QuorumHealth::Bad {
                return HttpResponse::ServiceUnavailable().finish();
            }
        }
    };

    HttpResponse::Ok().finish()
}

/// Redirects from root to the "real root" /auth/v1/
#[get("/")]
pub async fn redirect() -> impl Responder {
    HttpResponse::MovedPermanently()
        .insert_header((
            header::LOCATION,
            HeaderValue::from_str("/auth/v1/").unwrap(),
        ))
        .finish()
}

#[get("/v1")]
pub async fn redirect_v1() -> HttpResponse {
    HttpResponse::MovedPermanently()
        .insert_header(("location", "/auth/v1/"))
        .finish()
}

/// Simple `whoami` endpoint for debugging purposes
///
/// Returns a body with the original requests headers
#[utoipa::path(
    get,
    path = "/whoami",
    tag = "generic",
    params(WhoamiRequestParams),
    responses(
        (status = 200, description = "Ok"),
        (status = 503, description = "ServiceUnavailable"),
    ),
)]
#[get("/whoami")]
pub async fn whoami(req: HttpRequest, params: web::Query<WhoamiRequestParams>) -> impl Responder {
    use std::fmt::Write;

    let ip = real_ip_from_req(&req).unwrap_or_default();

    if let Some(typ) = &params.typ {
        match typ {
            WhoamiRequestParam::Ip => HttpResponse::Ok().append_header(HEADER_HTML).body(ip),
        }
    } else {
        let mut resp = String::with_capacity(500);

        req.headers().iter().for_each(|(k, v)| {
            let _ = writeln!(resp, "{}: {:?}", k, v.to_str().unwrap_or_default());
        });
        let _ = writeln!(resp, "ip: {}", ip);

        HttpResponse::Ok().body(resp)
    }
}

/// Returns the current Rauthy Version
#[utoipa::path(
    get,
    path = "/version",
    tag = "generic",
    responses(
        (status = 200, description = "Ok"),
    ),
)]
#[get("/version")]
pub async fn get_version(data: web::Data<AppState>) -> Result<HttpResponse, ErrorResponse> {
    let resp = match LatestAppVersion::find(&data).await {
        Some(latest) => {
            let update_available = match Version::from_str(RAUTHY_VERSION) {
                Ok(current) => latest.latest_version > current,
                Err(err) => {
                    error!("Cannot parse RAUTHY_VERSION: {:?}", err);
                    false
                }
            };
            AppVersionResponse {
                current: RAUTHY_VERSION.to_string(),
                last_check: Some(latest.timestamp),
                latest: Some(latest.latest_version.to_string()),
                latest_url: Some(latest.release_url.to_string()),
                update_available,
            }
        }
        None => AppVersionResponse {
            current: RAUTHY_VERSION.to_string(),
            last_check: None,
            latest: None,
            latest_url: None,
            update_available: false,
        },
    };
    Ok(HttpResponse::Ok().json(resp))
}
