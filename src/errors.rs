use rocket::http::Status;
use rocket::request::Request;
use rocket::response::{Responder, Response};
use rocket::serde::json::json;
use std::io::Cursor;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
	#[error("not found")] 
	NotFound,
	#[error("unauthorized")] 
	Unauthorized,
	#[error("forbidden")] 
	Forbidden,
	#[error("too many requests")] 
	TooManyRequests,
	#[error("bad request: {0}")] 
	BadRequest(String),
	#[error("conflict: {0}")] 
	Conflict(String),
	#[error(transparent)] 
	Sqlite(#[from] r2d2_sqlite::rusqlite::Error),
	#[error(transparent)] 
	Jwt(#[from] jsonwebtoken::errors::Error),
	#[error(transparent)] 
	Json(#[from] serde_json::Error),
	#[error(transparent)] 
	Anyhow(#[from] anyhow::Error),
}

impl AppError {
	pub fn status(&self) -> Status {
		match self {
			AppError::NotFound => Status::NotFound,
			AppError::Unauthorized => Status::Unauthorized,
			AppError::Forbidden => Status::Forbidden,
			AppError::TooManyRequests => Status::TooManyRequests,
			AppError::BadRequest(_) => Status::BadRequest,
			AppError::Conflict(_) => Status::Conflict,
			AppError::Sqlite(_) => Status::InternalServerError,
			AppError::Jwt(_) => Status::Unauthorized,
			AppError::Json(_) => Status::BadRequest,
			AppError::Anyhow(_) => Status::InternalServerError,
		}
	}
}

impl<'r> Responder<'r, 'static> for AppError {
	fn respond_to(self, _req: &'r Request<'_>) -> Result<Response<'static>, Status> {
		let status = self.status();
		let body = json!({
			"error": self.to_string(),
			"code": status.code,
		});
		Response::build()
			.status(status)
			.sized_body(None, Cursor::new(body.to_string()))
			.header(rocket::http::ContentType::JSON)
			.ok()
	}
}

pub type AppResult<T> = Result<T, AppError>; 