mod app;
mod auth;
mod castboard;
mod device;
mod filesystem;
mod message;
mod music;
mod notes;
mod settings;
mod update;
mod terminal;
mod camera;
mod context_menu;

pub use app::bootstrap_app;
pub use auth::{
    clear_saved_login, get_saved_login, login, logout, register_account, save_saved_login,
};
pub use castboard::{
    castboard_request, get_castboard_status, handle_castboard_window_event, list_castboard_monitors,
    open_castboard_on_monitor, stop_castboard,
};
pub use device::{
    create_pair_string, delete_device, forget_lan_trust, list_devices, list_lan_pairing_candidates,
    refresh_devices,
    respond_lan_pairing, rotate_device_key, start_lan_pairing, update_device_name,
};
pub use message::{
    cancel_transfer, clear_transfers, open_received_file, pending_file_offers, pick_files,
    respond_file_offer, reveal_received_file, send_files, send_text,
};
pub use filesystem::{
    download_remote_filesystem_file, upload_remote_filesystem_file, list_remote_filesystem, list_remote_filesystem_downloads, list_remote_filesystem_uploads,
    list_remote_filesystem_roots,
};
pub use music::{get_music_providers, list_available_music_providers, update_music_providers};
pub use notes::{
    notes_attachments_delete, notes_attachments_list, notes_attachments_open, notes_attachments_resolve_path,
    notes_attachments_stage, notes_delete, notes_get, notes_list, notes_new_id,
    notes_resolve_conflict, notes_storage, notes_sync, notes_tags_create, notes_tags_delete,
    notes_tags_list, notes_tags_rename, notes_upsert,
};
pub use settings::{get_settings, pick_download_directory, update_settings};
pub use update::{check_update, install_tauri_update, open_update_download};
pub use terminal::{close_terminal, get_remote_terminal_support, open_terminal, resize_terminal, write_terminal};
pub use camera::{close_remote_camera, get_remote_camera_support, list_remote_cameras, open_remote_camera, send_camera_alive};
pub use context_menu::{get_pending_share_files, parse_send_args, SYSTEM_SHARE_FILES_EVENT};
