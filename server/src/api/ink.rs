use super::*;
use notes_core::ink::transfer::{self, Blobs};
use notes_protocol::ink::{self as wire, Hashes};

pub(super) async fn missing(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<Hashes>,
) -> Result<Json<Hashes>, ApiError> {
    request
        .validate()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let owned = state
        .blob_ownership
        .owned(&user.0.id, request.hashes.clone())
        .await
        .map_err(ApiError::internal)?;
    let store = BlobStore::new(state.data_dir);
    let maximum = state.max_blob_bytes;
    let hashes = tokio::task::spawn_blocking(move || {
        request
            .hashes
            .into_iter()
            .filter(|hash| {
                !owned.contains_key(hash) || store.open_verified(*hash, maximum).is_err()
            })
            .collect()
    })
    .await
    .map_err(|e| ApiError::internal(e.into()))?;
    Ok(Json(Hashes { hashes }))
}
pub(super) async fn upload(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    body: Body,
) -> Result<StatusCode, ApiError> {
    let bytes = axum::body::to_bytes(body, wire::MAX_BATCH_BYTES)
        .await
        .map_err(|_| ApiError::too_large("Ink batch too large"))?;
    let blobs = tokio::task::spawn_blocking(move || wire::decode(&bytes))
        .await
        .map_err(|e| ApiError::internal(e.into()))?
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    for (hash, bytes) in blobs {
        if bytes.len() as u64 > state.max_blob_bytes {
            return Err(ApiError::too_large("Ink blob too large"));
        }
        let claim = state
            .blob_ownership
            .claim(
                &user.0.id,
                hash,
                bytes.len() as u64,
                state.max_user_blob_bytes,
            )
            .await
            .map_err(map_blob_ownership_error)?;
        let store = BlobStore::new(state.data_dir.clone());
        let maximum = state.max_blob_bytes;
        let installed = tokio::task::spawn_blocking(move || {
            store.install_reader(bytes.as_slice(), hash, maximum)
        })
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
        if let Err(error) = installed {
            if claim == crate::blob_ownership::ClaimOutcome::Claimed {
                state
                    .blob_ownership
                    .release(&user.0.id, hash)
                    .await
                    .map_err(ApiError::internal)?;
            }
            return Err(map_blob_install_error(error));
        }
    }
    Ok(StatusCode::CREATED)
}
pub(super) async fn download(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<Hashes>,
) -> Result<Response, ApiError> {
    request
        .validate()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let owned = state
        .blob_ownership
        .owned(&user.0.id, request.hashes.clone())
        .await
        .map_err(ApiError::internal)?;
    let size = request
        .hashes
        .iter()
        .try_fold(12u64, |n, h| {
            owned.get(h).and_then(|size| n.checked_add(*size + 36))
        })
        .ok_or_else(|| ApiError::not_found("Ink blob not found"))?;
    if size > wire::MAX_BATCH_BYTES as u64 {
        return Err(ApiError::too_large("Ink download batch too large"));
    }
    let store = BlobStore::new(state.data_dir);
    let maximum = state.max_blob_bytes;
    let bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let mut blobs = Blobs::new();
        for hash in request.hashes {
            blobs.insert(hash, store.read_verified(hash, maximum)?);
        }
        Ok(wire::encode(&blobs)?)
    })
    .await
    .map_err(|e| ApiError::internal(e.into()))?
    .map_err(ApiError::internal)?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    )
        .into_response())
}
/// Every referenced blob must belong to this user, even if another user's file
/// with the same hash happens to exist in the shared physical store.
pub(super) async fn validate(
    state: &AppState,
    user: &UserState,
    roots: Vec<BlobHash>,
) -> Result<(), ApiError> {
    let roots = roots.into_iter().collect::<std::collections::BTreeSet<_>>();
    for hash in roots {
        let owned = state
            .blob_ownership
            .owned(&user.id, vec![hash])
            .await
            .map_err(ApiError::internal)?;
        if !owned.contains_key(&hash) {
            return Err(ApiError::conflict(
                "Upload handwriting root before publication",
            ));
        }
        let store = BlobStore::new(state.data_dir.clone());
        let root = tokio::task::spawn_blocking(move || {
            store.read_verified(hash, transfer::MAX_ROOT_BYTES)
        })
        .await
        .map_err(|e| ApiError::internal(e.into()))?
        .map_err(|e| map_declared_blob_error(hash, e))?;
        let required = transfer::required_blobs(hash, &root)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        let owned = state
            .blob_ownership
            .owned(&user.id, required.keys().copied().collect())
            .await
            .map_err(ApiError::internal)?;
        if required
            .iter()
            .any(|(hash, size)| owned.get(hash) != Some(size))
        {
            return Err(ApiError::conflict(
                "Upload all handwriting blocks before publication",
            ));
        }
        let store = BlobStore::new(state.data_dir.clone());
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let mut blobs = Blobs::new();
            blobs.insert(hash, root);
            for (hash, size) in required {
                blobs.insert(hash, store.read_verified(hash, size)?);
            }
            transfer::validate_graph(hash, &blobs)?;
            Ok(())
        })
        .await
        .map_err(|e| ApiError::internal(e.into()))?
        .map_err(|_| ApiError::bad_request("Invalid handwriting graph"))?;
    }
    Ok(())
}
