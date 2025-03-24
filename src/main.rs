mod auth_utils;
mod sync_utils;

use std::{fs, os::unix::fs::MetadataExt, path::PathBuf};

use auth_utils::authorise;
use rocket::{
    config::TlsConfig,
    fs::{FileServer, Options},
    http::Status,
    request::{FromRequest, Outcome},
    response::{status::BadRequest, Redirect},
    Request,
};
use sync_utils::{get_first_sync_data, ClientFileResponse, SyncInitialiserData, SyncManager};
use ws::Message;

#[macro_use]
extern crate rocket;

pub const LAST_SYNCED_SERVER_LIST_FILENAME_SUFFIX: &str = "_last_used_server_file_list.json";
pub const CLIENT_SYNC_INITIALISER_FILENAME_SUFFIX: &str = "_sync_initialiser.json";
pub const CLIENT_FOLDER_LIST_FILENAME_SUFFIX: &str = "_sync_folder_list.json";
pub const PREVIOUS_PREFIX: &str = "previous_";
pub const LORO_NOTE_ID: &str = "LORO_NOTE_ID";
pub const IDIRFEIN_ROOT_FOLDER_ENV_VAR: &str = "IDIRFEIN_ROOT_FOLDER";

#[get("/", rank = 2)]
fn redirect_to_blog() -> Redirect {
    Redirect::permanent(uri!("/blog/index.html"))
}

struct AuthToken();

#[derive(Debug)]
enum AuthTokenError {
    Missing,
    Invalid,
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for AuthToken {
    type Error = AuthTokenError;

    async fn from_request(req: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        if let Some(Ok(client_id)) = req.query_value::<&str>("client_id") {
            match req
                .headers()
                .get_one("Authorization")
                .and_then(|bearer_auth_token| bearer_auth_token.strip_prefix("Bearer "))
            {
                None => Outcome::Error((Status::BadRequest, AuthTokenError::Missing)),
                Some(auth_token) if authorise(client_id, auth_token) => {
                    Outcome::Success(AuthToken())
                }
                Some(_) => Outcome::Error((Status::Unauthorized, AuthTokenError::Invalid)),
            }
        } else {
            Outcome::Error((Status::BadRequest, AuthTokenError::Missing))
        }
    }
}

struct IsInitialised(SyncInitialiserData);

#[derive(Debug)]
enum IsInitialisedError {
    NoClientId,
    MissingInitialisationData,
    TooOld,
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for IsInitialised {
    type Error = IsInitialisedError;

    async fn from_request(req: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        if let Some(Ok(client_id)) = req.query_value::<&str>("client_id") {
            let sync_data_path = PathBuf::from(
                std::env::var(IDIRFEIN_ROOT_FOLDER_ENV_VAR)
                    .unwrap_or(String::from("/mnt/idirfein_data/idirfein_sync_data")),
            )
            .join(format!(
                "{client_id}{CLIENT_SYNC_INITIALISER_FILENAME_SUFFIX}"
            ));
            if sync_data_path.exists() {
                if sync_data_path.metadata().is_ok_and(|metadata| {
                    (std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                        - metadata.mtime().unsigned_abs())
                        < 1200 // Can't be older than 20 minutes
                }) {
                    let initialiser_data_result = fs::read_to_string(sync_data_path);
                    if let Ok(initialiser_data) = initialiser_data_result {
                        let deserialised_option =
                            serde_json::from_str::<SyncInitialiserData>(&initialiser_data);
                        match deserialised_option {
                            Ok(deserialised) => Outcome::Success(IsInitialised(deserialised)),
                            Err(_) => Outcome::Error((
                                Status::NotFound,
                                IsInitialisedError::MissingInitialisationData,
                            )),
                        }
                    } else {
                        Outcome::Error((
                            Status::NotFound,
                            IsInitialisedError::MissingInitialisationData,
                        ))
                    }
                } else {
                    Outcome::Error((Status::NotAcceptable, IsInitialisedError::TooOld))
                }
            } else {
                Outcome::Error((
                    Status::NotFound,
                    IsInitialisedError::MissingInitialisationData,
                ))
            }
        } else {
            Outcome::Error((Status::BadRequest, IsInitialisedError::NoClientId))
        }
    }
}

#[get("/first_sync?<client_id>", rank = 1)]
fn first_sync(
    client_id: &str,
    _auth_token: AuthToken,
) -> Result<Vec<u8>, rocket::response::status::Custom<&str>> {
    if let Ok(response_bytes) = serde_json::to_vec(&get_first_sync_data()) {
        Ok(response_bytes)
    } else {
        Err(rocket::response::status::Custom(
            Status::InternalServerError,
            "Couldn't serialise sync data",
        ))
    }
}

#[post("/initialise?<client_id>", data = "<initialiser>", rank = 1)]
fn initialise_sync(
    client_id: &str,
    initialiser: &str,
    _auth_token: AuthToken,
) -> Result<Vec<u8>, BadRequest<String>> {
    if let Ok(initialiser_data) = serde_json::from_str::<SyncInitialiserData>(initialiser) {
        let sync_manager = SyncManager::new(client_id, initialiser_data);
        if let Ok(serialised_initialiser_data) =
            serde_json::to_string(&sync_manager.initialiser_data)
        {
            let _ = fs::write(
                PathBuf::from(
                    std::env::var(IDIRFEIN_ROOT_FOLDER_ENV_VAR)
                        .unwrap_or(String::from("/mnt/idirfein_data/idirfein_sync_data")),
                )
                .join(format!(
                    "{client_id}{CLIENT_SYNC_INITIALISER_FILENAME_SUFFIX}"
                )),
                &serialised_initialiser_data,
            );
            if let Ok(response_bytes) = serde_json::to_vec(&sync_manager.compare_remote_file_list())
            {
                return Ok(response_bytes);
            }
        }
    }
    Err(BadRequest(String::from("Invalid JSON")))
}

#[get("/stream?<client_id>", rank = 1)]
fn sync_channel(
    ws: ws::WebSocket,
    client_id: String,
    _auth_token: AuthToken,
    initialiser_data: IsInitialised,
) -> ws::Channel<'static> {
    use rocket::futures::{SinkExt, StreamExt};

    let sync_manager = SyncManager::new(&client_id, initialiser_data.0);
    ws.channel(move |mut stream| {
        Box::pin(async move {
            while let Some(message_result) = stream.next().await {
                if let Some(client_file_response) = message_result
                    .ok()
                    .and_then(|message| message.into_text().ok())
                    .and_then(|message_text| {
                        serde_json::from_str::<ClientFileResponse>(&message_text).ok()
                    })
                {
                    if let Some(server_response) =
                        sync_manager.handle_client_file_response(client_file_response)
                    {
                        if let Ok(server_response_json) = serde_json::to_string(&server_response) {
                            let _ = stream.send(Message::text(server_response_json)).await;
                        }
                    }
                }
            }
            let _ = fs::write(
                PathBuf::from(
                    std::env::var(IDIRFEIN_ROOT_FOLDER_ENV_VAR)
                        .unwrap_or(String::from("/mnt/idirfein_data/idirfein_sync_data")),
                )
                .join(format!(
                    "{PREVIOUS_PREFIX}{client_id}{LAST_SYNCED_SERVER_LIST_FILENAME_SUFFIX}"
                )),
                serde_json::to_string(&sync_manager.server_file_list).unwrap(),
            );
            let _ = fs::write(
                PathBuf::from(
                    std::env::var(IDIRFEIN_ROOT_FOLDER_ENV_VAR)
                        .unwrap_or(String::from("/mnt/idirfein_data/idirfein_sync_data")),
                )
                .join(format!(
                    "{PREVIOUS_PREFIX}{client_id}{CLIENT_FOLDER_LIST_FILENAME_SUFFIX}"
                )),
                serde_json::to_string(&sync_manager.list_of_folder_ids).unwrap(),
            );

            Ok(())
        })
    })
}

#[launch]
fn rocket() -> _ {
    let _ = fs::create_dir_all(
        PathBuf::from(
            std::env::var(IDIRFEIN_ROOT_FOLDER_ENV_VAR)
                .unwrap_or(String::from("/mnt/idirfein_data/idirfein_sync_data")),
        )
        .join("web_data")
        .join("www"),
    );

    rocket::build()
        .configure(rocket::Config {
            address: "0.0.0.0".parse().expect("Can't fail"),
            port: 8000,
            tls: Some(TlsConfig::from_paths(
                "/etc/letsencrypt/live/idirfein.duckdns.org/fullchain.pem",
                "/etc/letsencrypt/live/idirfein.duckdns.org/privkey.pem",
            )),
            ..Default::default()
        })
        .mount("/", routes![redirect_to_blog])
        .mount("/sync", routes![sync_channel, initialise_sync, first_sync])
        .mount(
            "/blog",
            FileServer::new(
                PathBuf::from(
                    std::env::var(IDIRFEIN_ROOT_FOLDER_ENV_VAR)
                        .unwrap_or(String::from("/mnt/idirfein_data/idirfein_sync_data")),
                )
                .join("web_data")
                .join("www"),
                Options::default(),
            ),
        )
}
