use crate::CLIENT_FILE_LIST_FILENAME_SUFFIX;
use crate::CLIENT_FOLDER_LIST_FILENAME_SUFFIX;
use crate::PREVIOUS_PREFIX;
use fast_rsync::Signature;
use fast_rsync::SignatureOptions;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::{fs, os::unix::fs::MetadataExt, path::PathBuf};
use walkdir::WalkDir;

use crate::ROOT_FOLDER_NAME;

const DEFAULT_CRYPTO_HASH_SIZE: u32 = 16;

pub fn get_first_sync_data() -> HashMap<String, Vec<String>> {
    let mut list_of_paths_dict: HashMap<String, Vec<String>> = HashMap::new();

    let root_dir = dirs::home_dir()
        .expect("No Home dir found, very strange, what weird OS are you running?")
        .join(ROOT_FOLDER_NAME);
    let list_of_folder_ids: Vec<String> = fs::read_dir(&root_dir)
        .unwrap()
        .filter_map(|item| item.ok().map(|item| item.path()))
        .filter(|item| item.is_dir())
        .map(|item| item.file_name().unwrap().to_str().unwrap().to_string())
        .collect();
    list_of_folder_ids.into_iter().for_each(|folder_id| {
        let mut all_child_paths: Vec<String> = WalkDir::new(root_dir.join(&folder_id))
            .into_iter()
            .filter_map(|dir_entry_result| dir_entry_result.ok())
            .map(|dir_entry| dir_entry.into_path())
            .filter(|filepath| filepath.is_file())
            .map(|filepath| {
                filepath
                    .strip_prefix(root_dir.join(&folder_id))
                    .expect("Path should be child of folder path")
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        all_child_paths.sort_unstable();
        list_of_paths_dict.insert(folder_id, all_child_paths);
    });
    list_of_paths_dict
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerFileRequest {
    /// The file at the given index is being requested by the remote server
    RequestNewFile(usize),
    /// This new folder from the server should be created on the client
    CreateFolder(String),
    /// The signature of the file at the given index is being requested so it can create a diff to update it
    RequestSignature(usize),
    /// The client should apply the given diff to the file at the given index
    ApplyDiff(usize, Vec<u8>),
    /// The diff between the file at the given index and the signature made up from these bytes is being requested to update the server file
    RequestDiff(usize, Vec<u8>),
    /// The given file data should be written to the given relative path in the given folder id
    CreateFile(Vec<u8>, PathBuf, String),
    /// The file at the given index should be deleted on the client
    Delete(usize),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientFileResponse {
    /// The response to RequestNewFile
    NewFile(usize, Vec<u8>),
    /// The response to RequestSignature
    Signature(usize, Vec<u8>),
    /// The response to RequestDiff
    Diff(usize, Vec<u8>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filedata {
    /// The unique ID of the folder which contains this file
    pub folder_id: String,
    /// The relative path of this file from the root of its base folder
    pub relative_path: PathBuf,
    /// The total size of this file in bytes
    pub size: u64,
    /// The last modification of the file in seconds since the Unix epoch
    pub modification_time: i64,
    /// Whether to delete the file on the remote server
    pub should_delete: bool,
}

pub struct SyncManager {
    pub root_dir: PathBuf,
    pub list_of_folder_ids: Vec<String>,
    pub server_file_list: Vec<Filedata>,
    pub previously_synced_server_file_list: Vec<Filedata>,
    pub previously_synced_list_of_folder_ids: Vec<String>,
    pub client_file_list: Vec<Filedata>,
    pub client_ignore_list: Vec<String>,
}

impl SyncManager {
    pub fn new(
        client_id: &str,
        client_file_list: Vec<Filedata>,
        client_ignore_list: Vec<String>,
    ) -> Self {
        let root_dir = dirs::home_dir()
            .expect("No Home dir found, very strange, what weird OS are you running?")
            .join(ROOT_FOLDER_NAME);
        let list_of_folder_ids = fs::read_dir(&root_dir)
            .unwrap()
            .filter_map(|item| item.ok().map(|item| item.path()))
            .filter(|item| item.is_dir())
            .map(|item| item.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        let previously_synced_server_file_list = serde_json::from_str(
            &fs::read_to_string(
                dirs::home_dir()
                    .expect("No Home dir found, very strange, what weird OS are you running?")
                    .join(ROOT_FOLDER_NAME)
                    .join(format!(
                        "{PREVIOUS_PREFIX}{client_id}{CLIENT_FILE_LIST_FILENAME_SUFFIX}"
                    )),
            )
            .unwrap_or_default(),
        )
        .unwrap_or_default();
        let previously_synced_list_of_folder_ids = serde_json::from_str(
            &fs::read_to_string(
                dirs::home_dir()
                    .expect("No Home dir found, very strange, what weird OS are you running?")
                    .join(ROOT_FOLDER_NAME)
                    .join(format!(
                        "{PREVIOUS_PREFIX}{client_id}{CLIENT_FOLDER_LIST_FILENAME_SUFFIX}"
                    )),
            )
            .unwrap_or_default(),
        )
        .unwrap_or_default();
        let mut sync_manager = Self {
            root_dir,
            list_of_folder_ids,
            server_file_list: vec![],
            client_file_list,
            client_ignore_list,
            previously_synced_server_file_list,
            previously_synced_list_of_folder_ids,
        };
        sync_manager.get_server_file_list();
        sync_manager
    }

    pub fn get_server_file_list(&mut self) {
        self.server_file_list = self
            .list_of_folder_ids
            .iter()
            .flat_map(|folder_id| {
                let mut all_filedata: Vec<Filedata> = WalkDir::new(self.root_dir.join(folder_id))
                    .into_iter()
                    .filter_map(|dir_entry_result| dir_entry_result.ok())
                    .map(|dir_entry| dir_entry.into_path())
                    // Check ignore list
                    .filter(|filepath| {
                        filepath.to_str().is_some_and(|path_str| {
                            !self
                                .client_ignore_list
                                .iter()
                                .any(|ignore_list_item| path_str.contains(ignore_list_item))
                        }) && filepath.metadata().is_ok()
                    })
                    .map(|filepath| {
                        let metadata = filepath.metadata().unwrap();
                        // Truncate file path to get path relative to folder root
                        let relative_path = filepath
                            .strip_prefix(self.root_dir.join(folder_id))
                            .expect("Path should be child of folder path")
                            .to_path_buf();
                        Filedata {
                            folder_id: folder_id.clone(),
                            relative_path,
                            size: metadata.size(),
                            modification_time: metadata.mtime(),
                            should_delete: false,
                        }
                    })
                    .collect();
                all_filedata.sort_unstable_by(|a, b| a.relative_path.cmp(&b.relative_path));
                all_filedata.sort_by(|a, b| a.folder_id.cmp(&b.folder_id));
                all_filedata
            })
            .collect();
        // Get list of items that are in the list from the last sync with this device, but not in the new list, as these need to be deleted from the server, and add them to the new list to be deleted
        self.previously_synced_server_file_list
            .iter()
            .filter(|file_data_to_check| {
                !self.server_file_list.iter().any(|current_file_data| {
                    current_file_data.folder_id == file_data_to_check.folder_id
                        && current_file_data.relative_path == file_data_to_check.relative_path
                })
            })
            .cloned()
            .for_each(|mut item_to_delete| {
                item_to_delete.should_delete = true;
                self.client_file_list.push(item_to_delete);
            });
        println!("Server file list {:?}", self.server_file_list);
    }

    pub fn compare_remote_file_list(&self) -> Vec<ServerFileRequest> {
        let mut requests_to_send = vec![];
        self.list_of_folder_ids
            .iter()
            .filter(|new_id| !self.previously_synced_list_of_folder_ids.contains(new_id))
            .for_each(|id_to_create| {
                // Create each new server folder on the client
                requests_to_send.push(ServerFileRequest::CreateFolder(id_to_create.clone()));
            });
        self.client_file_list.iter().for_each(|client_file| {
            if !self.list_of_folder_ids.contains(&client_file.folder_id) {
                let _ = fs::create_dir_all(self.root_dir.join(&client_file.folder_id));
            }
            if client_file.should_delete {
                self.delete_file_on_server(client_file);
            }
        });
        self.client_file_list
            .iter()
            .enumerate()
            .for_each(|(client_index, client_file)| {
                // Check for matching file on server
                if let Some(server_file) = self.server_file_list.iter().find(|server_file| {
                    server_file.folder_id == client_file.folder_id
                        && server_file.relative_path == client_file.relative_path
                }) {
                    if server_file.should_delete {
                        // If server file is marked as deleted, delete it from the client
                        requests_to_send.push(ServerFileRequest::Delete(client_index));
                    } else if client_file.size != server_file.size {
                        if client_file.modification_time >= server_file.modification_time {
                            // If size differs and client file is newer, request a diff
                            requests_to_send.push(ServerFileRequest::RequestDiff(
                                client_index,
                                Signature::calculate(
                                    &self.get_file_bytes(server_file),
                                    SignatureOptions {
                                        block_size: get_ideal_block_size(server_file.size),
                                        crypto_hash_size: DEFAULT_CRYPTO_HASH_SIZE,
                                    },
                                )
                                .into_serialized(),
                            ));
                        } else {
                            requests_to_send
                                // If size differs and server file is newer, request a signature to calculate a diff
                                .push(ServerFileRequest::RequestSignature(client_index));
                        }
                    }
                } else {
                    // If none found, request the file from the client
                    requests_to_send.push(ServerFileRequest::RequestNewFile(client_index));
                }
            });
        self.server_file_list.iter().for_each(|server_file| {
            // Check for matching file on server
            if !self.server_file_list.iter().any(|client_file| {
                client_file.folder_id == server_file.folder_id
                    && client_file.relative_path == server_file.relative_path
            }) {
                // If server file can't be found in client list, send to client
                requests_to_send.push(ServerFileRequest::CreateFile(
                    self.get_file_bytes(server_file),
                    server_file.relative_path.clone(),
                    server_file.folder_id.clone(),
                ));
            }
        });

        requests_to_send
    }

    pub fn handle_client_file_response(
        &self,
        client_file_response: ClientFileResponse,
    ) -> Option<ServerFileRequest> {
        match client_file_response {
            ClientFileResponse::NewFile(file_index, file_bytes) => {
                if let Some(client_file) = self.client_file_list.get(file_index) {
                    let _ = fs::write(
                        self.root_dir
                            .join(&client_file.folder_id)
                            .join(&client_file.relative_path),
                        file_bytes,
                    );
                }
            }
            ClientFileResponse::Signature(file_index, signature_bytes) => {
                if let Some(client_file) = self.client_file_list.get(file_index) {
                    if let Ok(signature) = Signature::deserialize(signature_bytes) {
                        let server_file_bytes = self.get_file_bytes(client_file);
                        let mut diff: Vec<u8> = vec![];
                        if fast_rsync::diff(&signature.index(), &server_file_bytes, &mut diff)
                            .is_ok()
                        {
                            return Some(ServerFileRequest::ApplyDiff(file_index, diff));
                        }
                    }
                }
            }
            ClientFileResponse::Diff(file_index, diff_bytes) => {
                if let Some(client_file) = self.client_file_list.get(file_index) {
                    let server_file_bytes = self.get_file_bytes(client_file);
                    let mut server_file_handle = File::create(
                        self.root_dir
                            .join(&client_file.folder_id)
                            .join(&client_file.relative_path),
                    )
                    .unwrap();
                    let _ =
                        fast_rsync::apply(&server_file_bytes, &diff_bytes, &mut server_file_handle);
                }
            }
        }
        None
    }

    pub fn delete_file_on_server(&self, file_to_delete: &Filedata) {
        let _ = fs::remove_file(
            self.root_dir
                .join(&file_to_delete.folder_id)
                .join(&file_to_delete.relative_path),
        );
    }

    pub fn get_file_bytes(&self, file_to_read: &Filedata) -> Vec<u8> {
        fs::read(
            self.root_dir
                .join(&file_to_read.folder_id)
                .join(&file_to_read.relative_path),
        )
        .unwrap()
    }
}

/// Get the ideal block size for a given file, based off the rsync implementation https://git.samba.org/?p=rsync.git;a=blob;f=generator.c;h=5538a92dd57ddf8671a2404a7308ada73a710f58;hb=HEAD#l600
pub fn get_ideal_block_size(file_size: u64) -> u32 {
    let sqrt = (file_size as f64).sqrt();
    let rounded = (sqrt / 8.0).round() * 8.0;
    rounded as u32
}
