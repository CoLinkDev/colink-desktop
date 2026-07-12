#[derive(Clone, Copy)]
pub enum TextKey {
    TrayOpen,
    TraySettings,
    TrayQuit,
    TrayCloud,
    TrayReachableDevices,
    TrayLan,
    CloudConnected,
    CloudConnecting,
    CloudReconnecting,
    CloudDisconnected,
    MessageEmpty,
    MessageTooLong,
    MessageFromTitle,
    PairingRequestTitle,
    PairingRequestBody,
    DeviceNameEmpty,
    CannotDeleteLocalDevice,
    DownloadPathEmpty,
    DownloadPathMustBeAbsolute,
    SettingsNotInitialized,
    NotLoggedIn,
    SelectFiles,
    FileNotFound,
    InvalidFileName,
    FileReceiveTitle,
    FileReceiveStarted,
    FileReceiveCompleteTitle,
    FileSaved,
    FileReceiveFailedTitle,
    FileChecksumFailed,
    DeviceNotConnected,
    PairingRequestMissing,
    PairingRequestEnded,
    LanDeviceNotFound,
    LanDeviceAddressInvalid,
    LanPeerNotConnected,
    LanPeerUnavailable,
    CloudNotConnected,
    CloudUnavailable,
    ClipboardUnsupported,
}

pub fn default_language_code() -> String {
    resolve_language(None).to_string()
}

pub fn resolve_language(selected: Option<&str>) -> &'static str {
    selected
        .and_then(normalize_locale)
        .or_else(system_language)
        .unwrap_or("en")
}

pub fn text(language: &str, key: TextKey) -> &'static str {
    let language = resolve_language(Some(language));
    match language {
        "zh-CN" => text_zh_cn(key),
        "zh-TW" => text_zh_tw(key),
        "ja" => text_ja(key),
        "ko" => text_ko(key),
        "es" => text_es(key),
        "de" => text_de(key),
        "ru" => text_ru(key),
        _ => text_en(key),
    }
}

pub fn message(language: &str, key: TextKey, args: &[(&str, String)]) -> String {
    let mut value = text(language, key).to_string();
    for (key, replacement) in args {
        value = value.replace(&format!("{{{key}}}"), replacement);
    }
    value
}

pub fn cloud_state(language: &str, state: &str) -> &'static str {
    match state {
        "connected" => text(language, TextKey::CloudConnected),
        "connecting" => text(language, TextKey::CloudConnecting),
        "reconnecting" => text(language, TextKey::CloudReconnecting),
        _ => text(language, TextKey::CloudDisconnected),
    }
}

fn system_language() -> Option<&'static str> {
    sys_locale::get_locale()
        .as_deref()
        .and_then(normalize_locale)
}

fn normalize_locale(raw: &str) -> Option<&'static str> {
    let value = raw.trim().replace('_', "-").to_ascii_lowercase();
    if value.is_empty() {
        return None;
    }

    if value == "zh" || value.starts_with("zh-cn") || value.starts_with("zh-hans") {
        return Some("zh-CN");
    }
    if value.starts_with("zh-tw")
        || value.starts_with("zh-hk")
        || value.starts_with("zh-mo")
        || value.starts_with("zh-hant")
    {
        return Some("zh-TW");
    }
    if value.starts_with("en") {
        return Some("en");
    }
    if value.starts_with("ja") {
        return Some("ja");
    }
    if value.starts_with("ko") {
        return Some("ko");
    }
    if value.starts_with("es") {
        return Some("es");
    }
    if value.starts_with("de") {
        return Some("de");
    }
    if value.starts_with("ru") {
        return Some("ru");
    }
    None
}

fn text_en(key: TextKey) -> &'static str {
    match key {
        TextKey::TrayOpen => "Open",
        TextKey::TraySettings => "Settings",
        TextKey::TrayQuit => "Quit",
        TextKey::TrayCloud => "Cloud",
        TextKey::TrayReachableDevices => "Reachable devices",
        TextKey::TrayLan => "LAN",
        TextKey::CloudConnected => "Connected",
        TextKey::CloudConnecting => "Connecting",
        TextKey::CloudReconnecting => "Reconnecting",
        TextKey::CloudDisconnected => "Disconnected",
        TextKey::MessageEmpty => "Message cannot be empty",
        TextKey::MessageTooLong => "Message cannot exceed 10000 characters",
        TextKey::MessageFromTitle => "Message from {name}",
        TextKey::PairingRequestTitle => "LAN pairing request",
        TextKey::PairingRequestBody => "{name} wants to pair. Code: {code}",
        TextKey::DeviceNameEmpty => "Device name cannot be empty",
        TextKey::CannotDeleteLocalDevice => "The local device cannot be deleted here",
        TextKey::DownloadPathEmpty => "File receiving path cannot be empty",
        TextKey::DownloadPathMustBeAbsolute => "File receiving path must be an absolute path",
        TextKey::SettingsNotInitialized => "Local settings are not initialized",
        TextKey::NotLoggedIn => "Not logged in",
        TextKey::SelectFiles => "Select files",
        TextKey::FileNotFound => "File does not exist: {path}",
        TextKey::InvalidFileName => "Invalid file name",
        TextKey::FileReceiveTitle => "File receiving",
        TextKey::FileReceiveStarted => "Receiving {file}",
        TextKey::FileReceiveCompleteTitle => "File received",
        TextKey::FileSaved => "Saved {file}",
        TextKey::FileReceiveFailedTitle => "File receive failed",
        TextKey::FileChecksumFailed => "{file} checksum verification failed",
        TextKey::DeviceNotConnected => "Device is not connected",
        TextKey::PairingRequestMissing => "Pairing request does not exist or has expired",
        TextKey::PairingRequestEnded => "Pairing request has ended",
        TextKey::LanDeviceNotFound => "LAN device was not found",
        TextKey::LanDeviceAddressInvalid => "Invalid LAN device address",
        TextKey::LanPeerNotConnected => "LAN peer is not connected",
        TextKey::LanPeerUnavailable => "LAN peer is unavailable",
        TextKey::CloudNotConnected => "Cloud connection is not established",
        TextKey::CloudUnavailable => "Cloud connection is unavailable",
        TextKey::ClipboardUnsupported => "Clipboard content is unsupported or exceeds 1 MB",
    }
}

fn text_zh_cn(key: TextKey) -> &'static str {
    match key {
        TextKey::TrayOpen => "打开",
        TextKey::TraySettings => "设置",
        TextKey::TrayQuit => "退出",
        TextKey::TrayCloud => "云端",
        TextKey::TrayReachableDevices => "可达设备",
        TextKey::TrayLan => "局域网",
        TextKey::CloudConnected => "已连接",
        TextKey::CloudConnecting => "连接中",
        TextKey::CloudReconnecting => "重连中",
        TextKey::CloudDisconnected => "未连接",
        TextKey::MessageEmpty => "消息不能为空",
        TextKey::MessageTooLong => "消息长度不能超过 10000",
        TextKey::MessageFromTitle => "来自 {name} 的消息",
        TextKey::PairingRequestTitle => "LAN 配对请求",
        TextKey::PairingRequestBody => "{name} 请求配对。配对码: {code}",
        TextKey::DeviceNameEmpty => "设备名称不能为空",
        TextKey::CannotDeleteLocalDevice => "本机设备不能在这里删除",
        TextKey::DownloadPathEmpty => "文件接收路径不能为空",
        TextKey::DownloadPathMustBeAbsolute => "文件接收路径必须是绝对路径",
        TextKey::SettingsNotInitialized => "本地设置未初始化",
        TextKey::NotLoggedIn => "尚未登录",
        TextKey::SelectFiles => "请选择文件",
        TextKey::FileNotFound => "文件不存在: {path}",
        TextKey::InvalidFileName => "文件名不合法",
        TextKey::FileReceiveTitle => "文件接收",
        TextKey::FileReceiveStarted => "开始接收 {file}",
        TextKey::FileReceiveCompleteTitle => "文件接收完成",
        TextKey::FileSaved => "已保存 {file}",
        TextKey::FileReceiveFailedTitle => "文件接收失败",
        TextKey::FileChecksumFailed => "{file} 校验失败",
        TextKey::DeviceNotConnected => "设备未连接",
        TextKey::PairingRequestMissing => "配对请求不存在或已过期",
        TextKey::PairingRequestEnded => "配对请求已结束",
        TextKey::LanDeviceNotFound => "未发现该 LAN 设备",
        TextKey::LanDeviceAddressInvalid => "LAN 设备地址无效",
        TextKey::LanPeerNotConnected => "LAN 对端未连接",
        TextKey::LanPeerUnavailable => "LAN 对端不可用",
        TextKey::CloudNotConnected => "云端连接尚未建立",
        TextKey::CloudUnavailable => "云端连接不可用",
        TextKey::ClipboardUnsupported => "剪贴板内容不支持或超过 1 MB",
    }
}

fn text_zh_tw(key: TextKey) -> &'static str {
    match key {
        TextKey::TrayOpen => "開啟",
        TextKey::TraySettings => "設定",
        TextKey::TrayQuit => "結束",
        TextKey::TrayCloud => "雲端",
        TextKey::TrayReachableDevices => "可達設備",
        TextKey::TrayLan => "區域網路",
        TextKey::CloudConnected => "已連線",
        TextKey::CloudConnecting => "連線中",
        TextKey::CloudReconnecting => "重新連線中",
        TextKey::CloudDisconnected => "未連線",
        TextKey::MessageEmpty => "訊息不能為空",
        TextKey::MessageTooLong => "訊息長度不能超過 10000",
        TextKey::MessageFromTitle => "來自 {name} 的訊息",
        TextKey::PairingRequestTitle => "LAN 配對請求",
        TextKey::PairingRequestBody => "{name} 請求配對。配對碼: {code}",
        TextKey::DeviceNameEmpty => "設備名稱不能為空",
        TextKey::CannotDeleteLocalDevice => "不能在這裡刪除本機設備",
        TextKey::DownloadPathEmpty => "檔案接收路徑不能為空",
        TextKey::DownloadPathMustBeAbsolute => "檔案接收路徑必須是絕對路徑",
        TextKey::SettingsNotInitialized => "本機設定尚未初始化",
        TextKey::NotLoggedIn => "尚未登入",
        TextKey::SelectFiles => "請選擇檔案",
        TextKey::FileNotFound => "檔案不存在: {path}",
        TextKey::InvalidFileName => "檔案名稱無效",
        TextKey::FileReceiveTitle => "檔案接收",
        TextKey::FileReceiveStarted => "開始接收 {file}",
        TextKey::FileReceiveCompleteTitle => "檔案接收完成",
        TextKey::FileSaved => "已儲存 {file}",
        TextKey::FileReceiveFailedTitle => "檔案接收失敗",
        TextKey::FileChecksumFailed => "{file} 校驗失敗",
        TextKey::DeviceNotConnected => "設備未連線",
        TextKey::PairingRequestMissing => "配對請求不存在或已過期",
        TextKey::PairingRequestEnded => "配對請求已結束",
        TextKey::LanDeviceNotFound => "找不到該 LAN 設備",
        TextKey::LanDeviceAddressInvalid => "LAN 設備位址無效",
        TextKey::LanPeerNotConnected => "LAN 對端未連線",
        TextKey::LanPeerUnavailable => "LAN 對端不可用",
        TextKey::CloudNotConnected => "雲端連線尚未建立",
        TextKey::CloudUnavailable => "雲端連線不可用",
        TextKey::ClipboardUnsupported => "剪貼簿內容不支援或超過 1 MB",
    }
}

fn text_ja(key: TextKey) -> &'static str {
    match key {
        TextKey::TrayOpen => "開く",
        TextKey::TraySettings => "設定",
        TextKey::TrayQuit => "終了",
        TextKey::TrayCloud => "クラウド",
        TextKey::TrayReachableDevices => "到達可能なデバイス",
        TextKey::TrayLan => "LAN",
        TextKey::CloudConnected => "接続済み",
        TextKey::CloudConnecting => "接続中",
        TextKey::CloudReconnecting => "再接続中",
        TextKey::CloudDisconnected => "未接続",
        TextKey::MessageEmpty => "メッセージを入力してください",
        TextKey::MessageTooLong => "メッセージは10000文字以内で入力してください",
        TextKey::MessageFromTitle => "{name} からのメッセージ",
        TextKey::PairingRequestTitle => "LAN ペアリング要求",
        TextKey::PairingRequestBody => "{name} がペアリングを要求しています。コード: {code}",
        TextKey::DeviceNameEmpty => "デバイス名を入力してください",
        TextKey::CannotDeleteLocalDevice => "ローカルデバイスはここでは削除できません",
        TextKey::DownloadPathEmpty => "ファイル受信先を入力してください",
        TextKey::DownloadPathMustBeAbsolute => "ファイル受信先は絶対パスで指定してください",
        TextKey::SettingsNotInitialized => "ローカル設定が初期化されていません",
        TextKey::NotLoggedIn => "ログインしていません",
        TextKey::SelectFiles => "ファイルを選択してください",
        TextKey::FileNotFound => "ファイルが存在しません: {path}",
        TextKey::InvalidFileName => "ファイル名が無効です",
        TextKey::FileReceiveTitle => "ファイル受信",
        TextKey::FileReceiveStarted => "{file} の受信を開始しました",
        TextKey::FileReceiveCompleteTitle => "ファイル受信完了",
        TextKey::FileSaved => "{file} を保存しました",
        TextKey::FileReceiveFailedTitle => "ファイル受信に失敗しました",
        TextKey::FileChecksumFailed => "{file} のチェックサム検証に失敗しました",
        TextKey::DeviceNotConnected => "デバイスが接続されていません",
        TextKey::PairingRequestMissing => "ペアリング要求が存在しないか期限切れです",
        TextKey::PairingRequestEnded => "ペアリング要求は終了しました",
        TextKey::LanDeviceNotFound => "LAN デバイスが見つかりません",
        TextKey::LanDeviceAddressInvalid => "LAN デバイスのアドレスが無効です",
        TextKey::LanPeerNotConnected => "LAN ピアが接続されていません",
        TextKey::LanPeerUnavailable => "LAN ピアを利用できません",
        TextKey::CloudNotConnected => "クラウド接続が確立されていません",
        TextKey::CloudUnavailable => "クラウド接続を利用できません",
        TextKey::ClipboardUnsupported => "クリップボード内容は未対応、または 1 MB を超えています",
    }
}

fn text_ko(key: TextKey) -> &'static str {
    match key {
        TextKey::TrayOpen => "열기",
        TextKey::TraySettings => "설정",
        TextKey::TrayQuit => "종료",
        TextKey::TrayCloud => "클라우드",
        TextKey::TrayReachableDevices => "연결 가능한 디바이스",
        TextKey::TrayLan => "LAN",
        TextKey::CloudConnected => "연결됨",
        TextKey::CloudConnecting => "연결 중",
        TextKey::CloudReconnecting => "다시 연결 중",
        TextKey::CloudDisconnected => "연결 안 됨",
        TextKey::MessageEmpty => "메시지를 입력해 주세요",
        TextKey::MessageTooLong => "메시지는 10000자를 넘을 수 없습니다",
        TextKey::MessageFromTitle => "{name}의 메시지",
        TextKey::PairingRequestTitle => "LAN 페어링 요청",
        TextKey::PairingRequestBody => "{name}에서 페어링을 요청했습니다. 코드: {code}",
        TextKey::DeviceNameEmpty => "디바이스 이름을 입력해 주세요",
        TextKey::CannotDeleteLocalDevice => "로컬 디바이스는 여기에서 삭제할 수 없습니다",
        TextKey::DownloadPathEmpty => "파일 수신 경로를 입력해 주세요",
        TextKey::DownloadPathMustBeAbsolute => "파일 수신 경로는 절대 경로여야 합니다",
        TextKey::SettingsNotInitialized => "로컬 설정이 초기화되지 않았습니다",
        TextKey::NotLoggedIn => "로그인되어 있지 않습니다",
        TextKey::SelectFiles => "파일을 선택해 주세요",
        TextKey::FileNotFound => "파일이 없습니다: {path}",
        TextKey::InvalidFileName => "파일 이름이 올바르지 않습니다",
        TextKey::FileReceiveTitle => "파일 받는 중",
        TextKey::FileReceiveStarted => "{file} 받기 시작",
        TextKey::FileReceiveCompleteTitle => "파일 받기 완료",
        TextKey::FileSaved => "{file} 저장됨",
        TextKey::FileReceiveFailedTitle => "파일 받기 실패",
        TextKey::FileChecksumFailed => "{file} 체크섬 검증 실패",
        TextKey::DeviceNotConnected => "디바이스가 연결되어 있지 않습니다",
        TextKey::PairingRequestMissing => "페어링 요청이 없거나 만료되었습니다",
        TextKey::PairingRequestEnded => "페어링 요청이 종료되었습니다",
        TextKey::LanDeviceNotFound => "LAN 디바이스를 찾을 수 없습니다",
        TextKey::LanDeviceAddressInvalid => "LAN 디바이스 주소가 올바르지 않습니다",
        TextKey::LanPeerNotConnected => "LAN 피어가 연결되어 있지 않습니다",
        TextKey::LanPeerUnavailable => "LAN 피어를 사용할 수 없습니다",
        TextKey::CloudNotConnected => "클라우드 연결이 아직 설정되지 않았습니다",
        TextKey::CloudUnavailable => "클라우드 연결을 사용할 수 없습니다",
        TextKey::ClipboardUnsupported => "클립보드 내용은 지원되지 않거나 1 MB를 초과합니다",
    }
}

fn text_es(key: TextKey) -> &'static str {
    match key {
        TextKey::TrayOpen => "Abrir",
        TextKey::TraySettings => "Ajustes",
        TextKey::TrayQuit => "Salir",
        TextKey::TrayCloud => "Nube",
        TextKey::TrayReachableDevices => "Dispositivos accesibles",
        TextKey::TrayLan => "LAN",
        TextKey::CloudConnected => "Conectado",
        TextKey::CloudConnecting => "Conectando",
        TextKey::CloudReconnecting => "Reconectando",
        TextKey::CloudDisconnected => "Desconectado",
        TextKey::MessageEmpty => "El mensaje no puede estar vacío",
        TextKey::MessageTooLong => "El mensaje no puede superar los 10000 caracteres",
        TextKey::MessageFromTitle => "Mensaje de {name}",
        TextKey::PairingRequestTitle => "Solicitud de emparejamiento LAN",
        TextKey::PairingRequestBody => "{name} quiere emparejarse. Código: {code}",
        TextKey::DeviceNameEmpty => "El nombre del dispositivo no puede estar vacío",
        TextKey::CannotDeleteLocalDevice => "El dispositivo local no se puede eliminar aquí",
        TextKey::DownloadPathEmpty => "La ruta de recepción de archivos no puede estar vacía",
        TextKey::DownloadPathMustBeAbsolute => "La ruta de recepción de archivos debe ser absoluta",
        TextKey::SettingsNotInitialized => "Los ajustes locales no están inicializados",
        TextKey::NotLoggedIn => "No has iniciado sesión",
        TextKey::SelectFiles => "Selecciona archivos",
        TextKey::FileNotFound => "El archivo no existe: {path}",
        TextKey::InvalidFileName => "Nombre de archivo no válido",
        TextKey::FileReceiveTitle => "Recepción de archivo",
        TextKey::FileReceiveStarted => "Recibiendo {file}",
        TextKey::FileReceiveCompleteTitle => "Archivo recibido",
        TextKey::FileSaved => "{file} guardado",
        TextKey::FileReceiveFailedTitle => "Error al recibir archivo",
        TextKey::FileChecksumFailed => "La verificación de {file} falló",
        TextKey::DeviceNotConnected => "El dispositivo no está conectado",
        TextKey::PairingRequestMissing => "La solicitud de emparejamiento no existe o expiró",
        TextKey::PairingRequestEnded => "La solicitud de emparejamiento finalizó",
        TextKey::LanDeviceNotFound => "No se encontró el dispositivo LAN",
        TextKey::LanDeviceAddressInvalid => "Dirección de dispositivo LAN no válida",
        TextKey::LanPeerNotConnected => "El par LAN no está conectado",
        TextKey::LanPeerUnavailable => "El par LAN no está disponible",
        TextKey::CloudNotConnected => "La conexión en la nube no está establecida",
        TextKey::CloudUnavailable => "La conexión en la nube no está disponible",
        TextKey::ClipboardUnsupported => {
            "El contenido del portapapeles no es compatible o supera 1 MB"
        }
    }
}

fn text_de(key: TextKey) -> &'static str {
    match key {
        TextKey::TrayOpen => "Öffnen",
        TextKey::TraySettings => "Einstellungen",
        TextKey::TrayQuit => "Beenden",
        TextKey::TrayCloud => "Cloud",
        TextKey::TrayReachableDevices => "Erreichbare Geräte",
        TextKey::TrayLan => "LAN",
        TextKey::CloudConnected => "Verbunden",
        TextKey::CloudConnecting => "Verbinden",
        TextKey::CloudReconnecting => "Wiederverbinden",
        TextKey::CloudDisconnected => "Getrennt",
        TextKey::MessageEmpty => "Nachricht darf nicht leer sein",
        TextKey::MessageTooLong => "Nachricht darf 10000 Zeichen nicht überschreiten",
        TextKey::MessageFromTitle => "Nachricht von {name}",
        TextKey::PairingRequestTitle => "LAN-Kopplungsanfrage",
        TextKey::PairingRequestBody => "{name} möchte koppeln. Code: {code}",
        TextKey::DeviceNameEmpty => "Gerätename darf nicht leer sein",
        TextKey::CannotDeleteLocalDevice => "Das lokale Gerät kann hier nicht gelöscht werden",
        TextKey::DownloadPathEmpty => "Dateiempfangspfad darf nicht leer sein",
        TextKey::DownloadPathMustBeAbsolute => "Dateiempfangspfad muss absolut sein",
        TextKey::SettingsNotInitialized => "Lokale Einstellungen sind nicht initialisiert",
        TextKey::NotLoggedIn => "Nicht angemeldet",
        TextKey::SelectFiles => "Dateien auswählen",
        TextKey::FileNotFound => "Datei existiert nicht: {path}",
        TextKey::InvalidFileName => "Ungültiger Dateiname",
        TextKey::FileReceiveTitle => "Dateiempfang",
        TextKey::FileReceiveStarted => "{file} wird empfangen",
        TextKey::FileReceiveCompleteTitle => "Datei empfangen",
        TextKey::FileSaved => "{file} gespeichert",
        TextKey::FileReceiveFailedTitle => "Dateiempfang fehlgeschlagen",
        TextKey::FileChecksumFailed => "Prüfsummenprüfung für {file} fehlgeschlagen",
        TextKey::DeviceNotConnected => "Gerät ist nicht verbunden",
        TextKey::PairingRequestMissing => "Kopplungsanfrage existiert nicht oder ist abgelaufen",
        TextKey::PairingRequestEnded => "Kopplungsanfrage wurde beendet",
        TextKey::LanDeviceNotFound => "LAN-Gerät wurde nicht gefunden",
        TextKey::LanDeviceAddressInvalid => "Ungültige LAN-Geräteadresse",
        TextKey::LanPeerNotConnected => "LAN-Peer ist nicht verbunden",
        TextKey::LanPeerUnavailable => "LAN-Peer ist nicht verfügbar",
        TextKey::CloudNotConnected => "Cloud-Verbindung ist nicht hergestellt",
        TextKey::CloudUnavailable => "Cloud-Verbindung ist nicht verfügbar",
        TextKey::ClipboardUnsupported => {
            "Zwischenablageinhalt wird nicht unterstützt oder überschreitet 1 MB"
        }
    }
}

fn text_ru(key: TextKey) -> &'static str {
    match key {
        TextKey::TrayOpen => "Открыть",
        TextKey::TraySettings => "Настройки",
        TextKey::TrayQuit => "Выйти",
        TextKey::TrayCloud => "Облако",
        TextKey::TrayReachableDevices => "Доступные устройства",
        TextKey::TrayLan => "LAN",
        TextKey::CloudConnected => "Подключено",
        TextKey::CloudConnecting => "Подключение",
        TextKey::CloudReconnecting => "Повторное подключение",
        TextKey::CloudDisconnected => "Отключено",
        TextKey::MessageEmpty => "Сообщение не может быть пустым",
        TextKey::MessageTooLong => "Сообщение не может превышать 10000 символов",
        TextKey::MessageFromTitle => "Сообщение от {name}",
        TextKey::PairingRequestTitle => "Запрос сопряжения LAN",
        TextKey::PairingRequestBody => "{name} хочет выполнить сопряжение. Код: {code}",
        TextKey::DeviceNameEmpty => "Имя устройства не может быть пустым",
        TextKey::CannotDeleteLocalDevice => "Локальное устройство нельзя удалить здесь",
        TextKey::DownloadPathEmpty => "Путь для получения файлов не может быть пустым",
        TextKey::DownloadPathMustBeAbsolute => "Путь для получения файлов должен быть абсолютным",
        TextKey::SettingsNotInitialized => "Локальные настройки не инициализированы",
        TextKey::NotLoggedIn => "Вход не выполнен",
        TextKey::SelectFiles => "Выберите файлы",
        TextKey::FileNotFound => "Файл не существует: {path}",
        TextKey::InvalidFileName => "Недопустимое имя файла",
        TextKey::FileReceiveTitle => "Получение файла",
        TextKey::FileReceiveStarted => "Получение {file}",
        TextKey::FileReceiveCompleteTitle => "Файл получен",
        TextKey::FileSaved => "{file} сохранен",
        TextKey::FileReceiveFailedTitle => "Не удалось получить файл",
        TextKey::FileChecksumFailed => "Проверка контрольной суммы {file} не удалась",
        TextKey::DeviceNotConnected => "Устройство не подключено",
        TextKey::PairingRequestMissing => "Запрос сопряжения не существует или истек",
        TextKey::PairingRequestEnded => "Запрос сопряжения завершен",
        TextKey::LanDeviceNotFound => "LAN-устройство не найдено",
        TextKey::LanDeviceAddressInvalid => "Недопустимый адрес LAN-устройства",
        TextKey::LanPeerNotConnected => "LAN-пир не подключен",
        TextKey::LanPeerUnavailable => "LAN-пир недоступен",
        TextKey::CloudNotConnected => "Облачное подключение не установлено",
        TextKey::CloudUnavailable => "Облачное подключение недоступно",
        TextKey::ClipboardUnsupported => {
            "Содержимое буфера обмена не поддерживается или превышает 1 MB"
        }
    }
}
