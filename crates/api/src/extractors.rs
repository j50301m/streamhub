use axum::extract::FromRequest;
use axum::extract::rejection::JsonRejection;
use axum::response::{IntoResponse, Response};
use common::AppError;
use serde::de::DeserializeOwned;

/// Custom JSON extractor that converts rejection errors into our unified error format.
pub struct AppJson<T>(pub T);

impl<S, T> FromRequest<S> for AppJson<T>
where
    axum::Json<T>: FromRequest<S, Rejection = JsonRejection>,
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(AppJson(value)),
            Err(rejection) => Err(AppError::Validation(rejection.body_text())),
        }
    }
}

impl<T: serde::Serialize> IntoResponse for AppJson<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}
