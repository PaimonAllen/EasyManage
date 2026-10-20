//! Administrator authorization is separate from ordinary Agent transport credentials.
use super::*;

pub(super) fn router() -> Router<OperationsState> {
    Router::new()
        .route(
            "/api/v1/nodes/{node_id}/admin-credentials",
            get(status).post(save).delete(forget),
        )
        .route(
            "/api/v1/platform/admin-credentials",
            get(platform_status)
                .post(platform_save)
                .delete(platform_forget),
        )
}

fn target(state: &OperationsState, id: Uuid) -> Result<Target, OperationsError> {
    load_targets(&state.registry)?
        .into_iter()
        .find(|t| t.node_id == id)
        .ok_or(OperationsError::bad_request("机器不在可信运维清单中"))
}

fn public_status(state: &OperationsState, target: &Target, headers: &HeaderMap) -> Response {
    let mut response =
        Json(json!({"available": ssh_vault::admin_available(&state.registry, target, headers)}))
            .into_response();
    response.headers_mut().insert(
        "cache-control",
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

async fn status(
    State(state): State<OperationsState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, OperationsError> {
    Ok(public_status(&state, &target(&state, id)?, &headers))
}
async fn platform_status(
    State(state): State<OperationsState>,
    headers: HeaderMap,
) -> Result<Response, OperationsError> {
    Ok(public_status(&state, &platform_target(&state)?, &headers))
}
async fn save(
    State(state): State<OperationsState>,
    Path(id): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
    Json(request): Json<Credentials>,
) -> Result<Response, OperationsError> {
    validate_origin(&headers, &uri)?;
    verify_and_save(state.clone(), target(&state, id)?, headers, request).await
}
async fn platform_save(
    State(state): State<OperationsState>,
    uri: Uri,
    headers: HeaderMap,
    Json(request): Json<Credentials>,
) -> Result<Response, OperationsError> {
    validate_origin(&headers, &uri)?;
    verify_and_save(state.clone(), platform_target(&state)?, headers, request).await
}

async fn verify_and_save(
    state: OperationsState,
    target: Target,
    headers: HeaderMap,
    request: Credentials,
) -> Result<Response, OperationsError> {
    let token = if request.use_saved {
        Some(ssh_vault::admin_token(&state.registry, &target, &headers)?)
    } else {
        None
    };
    let credentials = resolve_credentials(&state, &target, &headers, request)?;
    let lease = acquire_node_lease(&state.limits, target.node_id, true, true)?;
    let cancellation = Arc::new(Cancellation::default());
    let _guard = CancelOnDrop(cancellation.clone());
    let worker = tokio::task::spawn_blocking(move || {
        let _lease = lease;
        // Verify actual root authorization, not merely that SSH accepted a password/key.
        // No service restart, filesystem mutation or interactive shell is performed.
        let check = "/usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /usr/bin/timeout 10s /bin/sh -c 'test \"$(/usr/bin/id -u)\" = 0'";
        let verified = Credentials {
            username: credentials.username.clone(),
            password: credentials.password.clone(),
            private_key: credentials.private_key.clone(),
            passphrase: credentials.passphrase.clone(),
            sudo_password: credentials.sudo_password.clone(),
            use_saved: false,
        };
        run_privileged_command(&target, verified, &cancellation, check)?;
        if cancellation.is_cancelled() {
            return Err(OperationsError::ssh("管理员验证已取消，未保存新凭据"));
        }
        if let Some(token) = token {
            ssh_vault::save_admin_with_token(&state.registry, &target, &credentials, Some(token))
        } else {
            ssh_vault::save_admin(&state.registry, &target, &credentials)
        }
    });
    let cookie = tokio::time::timeout(Duration::from_secs(65), worker)
        .await
        .map_err(|_| OperationsError::ssh("管理员验证超时，未确认保存，请重试鉴权"))?
        .map_err(|_| OperationsError::ssh("管理员验证失败，未确认保存"))??;
    let mut response = Json(
        json!({"available": true, "message":"管理员身份已验证，凭据已加密保存；后续操作可复用"}),
    )
    .into_response();
    response.headers_mut().insert(
        "set-cookie",
        cookie.parse().map_err(|_| OperationsError::registry())?,
    );
    response.headers_mut().insert(
        "cache-control",
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

async fn forget(
    State(state): State<OperationsState>,
    Path(id): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, OperationsError> {
    validate_origin(&headers, &uri)?;
    remove(&state, &target(&state, id)?, &headers)
}
async fn platform_forget(
    State(state): State<OperationsState>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, OperationsError> {
    validate_origin(&headers, &uri)?;
    remove(&state, &platform_target(&state)?, &headers)
}
fn remove(
    state: &OperationsState,
    target: &Target,
    headers: &HeaderMap,
) -> Result<Response, OperationsError> {
    let _lease = acquire_node_lease(&state.limits, target.node_id, true, false)?;
    ssh_vault::forget_admin(&state.registry, target, headers)?;
    let mut response = Json(json!({"available":false})).into_response();
    response.headers_mut().insert(
        "set-cookie",
        format!(
            "em_admin_{}=; Path=/api/v1; Secure; HttpOnly; SameSite=Strict; Max-Age=0",
            target.node_id.simple()
        )
        .parse()
        .map_err(|_| OperationsError::registry())?,
    );
    response.headers_mut().insert(
        "cache-control",
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}
