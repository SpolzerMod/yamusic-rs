//! Persistent, event-driven player connection. No audio or UI operations run here.
use super::*;
use futures_util::{SinkExt, StreamExt};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use tokio_tungstenite::{connect_async_with_config, MaybeTlsStream as AsyncTls, WebSocketStream};

type Ws = WebSocketStream<AsyncTls<tokio::net::TcpStream>>;
const IO_TIMEOUT: Duration = Duration::from_secs(10);

pub enum Event {
    Status(String),
    Devices(String),
    State { state: Value, active: bool, changed: bool },
}

#[derive(Clone)]
struct Local {
    now: Option<Now>,
    volume: f32,
    claim: bool,
}

/// Watch channels coalesce snapshots: a broken network cannot accumulate queues.
pub struct Service {
    local: watch::Sender<Local>,
    stop: watch::Sender<bool>,
    thread: Option<std::thread::JoinHandle<()>>,
    events: Arc<Mutex<Vec<Event>>>,
}

impl Service {
    pub fn start(token: String, name: String, volume: f32, notify: impl Fn() + Send + Sync + 'static) -> Self {
        let (local, rx) = watch::channel(Local { now: None, volume, claim: false });
        let (stop, stopped) = watch::channel(false);
        let events = Arc::new(Mutex::new(Vec::<Event>::with_capacity(3)));
        let pending = events.clone();
        let event: Arc<dyn Fn(Event) + Send + Sync> = Arc::new(move |mut event| {
            let mut queue = pending.lock().unwrap();
            let wake = queue.is_empty();
            if let Some(old) = queue.iter_mut().find(|old| std::mem::discriminant(*old) == std::mem::discriminant(&event)) {
                if let (Event::State { state: previous, changed: was_changed, .. }, Event::State { state, changed, .. }) = (&*old, &mut event) {
                    *changed |= *was_changed;
                    if state.get("remote_volume").is_none() {
                        if let Some(v) = previous.get("remote_volume") { state["remote_volume"] = v.clone(); }
                    }
                }
                *old = event;
            } else { queue.push(event); }
            drop(queue);
            if wake { notify(); }
        });
        let thread = std::thread::Builder::new().name("ynison".into()).stack_size(512 * 1024).spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                event(Event::Status("Не удалось запустить Ynison".into()));
                return;
            };
            rt.block_on(async {
                let mut cancel = stopped.clone();
                tokio::select! {
                    _ = cancel.changed() => {}
                    _ = run(token, name, rx, stopped, event) => {}
                }
            });
            // Dropping the runtime cancels DNS/connect/read and closes all sockets.
        }).ok();
        Self { local, stop, thread, events }
    }

    pub fn take_events(&self) -> Vec<Event> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
    pub fn publish(&self, now: Option<Now>, volume: f32, claim: bool) {
        self.local.send_replace(Local { now, volume, claim });
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        // Never join a network thread on the audio thread. Cancellation above also
        // interrupts an outstanding write; runtime/socket destruction is asynchronous.
        self.thread.take();
    }
}

fn number(v: &Value) -> u64 {
    v.as_u64().or_else(|| v.as_str()?.parse().ok()).unwrap_or(0)
}

fn millis() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn version(id: &str) -> Value {
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = millis();
    let mut previous = LAST.load(std::sync::atomic::Ordering::Relaxed);
    let next = loop {
        let next = now.max(previous.saturating_add(1));
        match LAST.compare_exchange_weak(previous, next, std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed) {
            Ok(_) => break next,
            Err(actual) => previous = actual,
        }
    };
    json!({"device_id": id, "version": next.to_string(), "timestamp_ms": now.to_string()})
}

fn envelope(mut msg: Value, id: &str) -> Value {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    msg["rid"] = json!(format!("{id}-{}-{seq}", millis()));
    msg["player_action_timestamp_ms"] = json!(millis().to_string());
    msg["activity_interception_type"] = json!("DO_NOT_INTERCEPT_BY_DEFAULT");
    msg
}

fn hello(id: &str, name: &str, volume: f32) -> Value {
    let mut msg = shadow_hello(id);
    let full = &mut msg["update_full_state"];
    full["device"]["is_shadow"] = json!(false);
    full["device"]["capabilities"] = json!({
        "can_be_player": true, "can_be_remote_controller": false, "volume_granularity": 100
    });
    full["device"]["info"]["title"] = json!(name);
    full["device"]["volume_info"] = json!({"volume": volume, "version": version(id)});
    full["player_state"]["status"]["duration_ms"] = json!("0");
    full["player_state"]["status"]["progress_ms"] = json!("0");
    for field in ["status", "player_queue"] {
        full["player_state"][field]["version"] = json!({"device_id": id, "version": "0", "timestamp_ms": "0"});
    }
    envelope(msg, id)
}

fn request(url: &str, token: &str, protocol: &Value) -> Res<tungstenite::http::Request<()>> {
    let mut req = url.into_client_request().map_err(|_| "Некорректный адрес Ynison")?;
    let h = req.headers_mut();
    h.insert("Origin", HeaderValue::from_static("https://music.yandex.ru"));
    let mut auth = HeaderValue::from_str(&format!("OAuth {token}")).map_err(|_| "Некорректный токен")?;
    auth.set_sensitive(true);
    h.insert("Authorization", auth);
    h.insert("Sec-WebSocket-Protocol", HeaderValue::from_str(&format!("Bearer, v2, {protocol}")).map_err(|_| "Некорректный handshake")?);
    Ok(req)
}

async fn open_async(url: &str, token: &str, protocol: &Value) -> Res<Ws> {
    let config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(2 * 1024 * 1024))
        .max_frame_size(Some(2 * 1024 * 1024));
    let result = tokio::time::timeout(IO_TIMEOUT, connect_async_with_config(request(url, token, protocol)?, Some(config), false))
        .await.map_err(|_| "Таймаут подключения")?;
    result.map(|(ws, _)| ws).map_err(|e| match e {
        tungstenite::Error::Http(ref r) if r.status() == 401 || r.status() == 403 => "Требуется повторный вход".into(),
        _ => "Сеть Ynison недоступна".into(), // Never format handshake errors (headers contain credentials).
    })
}

async fn send(ws: &mut Ws, msg: Value) -> Res<()> {
    tokio::time::timeout(IO_TIMEOUT, ws.send(Message::text(msg.to_string()))).await
        .map_err(|_| "Таймаут отправки")?.map_err(|_| "Соединение прервано".into())
}

async fn read(ws: &mut Ws) -> Res<Value> {
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(t))) => return serde_json::from_str(&t).map_err(|_| "Некорректный ответ Ynison".into()),
            Some(Ok(Message::Ping(_))) => {
                ws.flush().await.map_err(|_| "Соединение прервано")?;
                return Ok(json!({}));
            }
            Some(Ok(Message::Pong(_))) => return Ok(json!({})),
            Some(Ok(Message::Binary(_))) => return Err("Неизвестный бинарный ответ".into()),
            _ => return Err("Соединение прервано".into()),
        }
    }
}

async fn run(token: String, name: String, mut local: watch::Receiver<Local>, mut stopped: watch::Receiver<bool>, event: Arc<dyn Fn(Event) + Send + Sync>) {
    // Separate identity from the legacy short-lived shadow reader used on startup.
    let id = format!("{}-connect", device_id());
    let mut attempt = 0usize;
    let mut backoff = vec![1000u64, 5000, 30000, 300000];
    loop {
        if *stopped.borrow() { return; }
        event(Event::Status("Подключение…".into()));
        let started = tokio::time::Instant::now();
        let result = session(&token, &id, &name, &mut local, &mut stopped, &event, &mut backoff).await;
        if *stopped.borrow() { return; }
        if started.elapsed() > Duration::from_secs(30) { attempt = 0; }
        let delay = match result {
            Ok(seconds) => { attempt = 0; seconds.saturating_mul(1000) }
            Err(ref e) if e == "Требуется повторный вход" || e == "Устройство вытеснено; выключите и включите Ynison" => {
                event(Event::Status(e.clone()));
                let _ = stopped.changed().await;
                return;
            }
            Err(_) => {
                let milliseconds = backoff[attempt.min(backoff.len() - 1)];
                attempt = attempt.saturating_add(1);
                milliseconds
            }
        };
        event(Event::Status(format!("Нет соединения; повтор через {} с", delay.div_ceil(1000))));
        event(Event::Devices(String::new()));
        tokio::select! {
            _ = stopped.changed() => return,
            _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
        }
    }
}

async fn session(token: &str, id: &str, name: &str, local: &mut watch::Receiver<Local>, stopped: &mut watch::Receiver<bool>, event: &Arc<dyn Fn(Event) + Send + Sync>, backoff: &mut Vec<u64>) -> Res<u64> {
    let bootstrap = async {
    let mut protocol = json!({"Ynison-Device-Id": id, "Ynison-Device-Info": json!({"app_name": "Chrome", "app_version": env!("CARGO_PKG_VERSION"), "type": 1}).to_string()});
    let mut redirect = open_async(REDIRECT, token, &protocol).await?;
    let to = tokio::time::timeout(IO_TIMEOUT, read(&mut redirect)).await.map_err(|_| "Таймаут redirector")??;
    drop(redirect);
    let host = to["host"].as_str().filter(|h| valid_host(h)).ok_or("Некорректный host Ynison")?;
    protocol["Ynison-Redirect-Ticket"] = to["redirect_ticket"].clone();
    protocol["Ynison-Session-Id"] = json!(to["session_id"].as_str().map(str::to_owned).unwrap_or_else(|| to["session_id"].to_string()));
    let mut ws = open_async(&format!("wss://{host}/ynison_state.YnisonStateService/PutYnisonState"), token, &protocol).await?;
    let initial = local.borrow_and_update().clone();
    send(&mut ws, hello(id, name, initial.volume)).await?;
    Ok::<_, String>((ws, to, initial))
    };
    let (mut ws, to, initial) = tokio::select! {
        _ = stopped.changed() => return Ok(0),
        connected = bootstrap => connected?,
    };
    event(Event::Status("Регистрация…".into()));
    let mut registered = false;
    let seconds = number(&to["keep_alive_params"]["keep_alive_time_seconds"]);
    let heartbeat = Duration::from_secs(if seconds == 0 { 30 } else { seconds.clamp(5, 300) });
    let mut ping = tokio::time::interval(heartbeat);
    ping.tick().await;
    let mut last_rx = tokio::time::Instant::now();
    let mut state = json!({});
    let mut active = false;
    let mut sent_volume = initial.volume;
    loop {
        tokio::select! {
            _ = stopped.changed() => {
                let _ = tokio::time::timeout(Duration::from_millis(500), ws.close(None)).await;
                return Ok(0);
            }
            incoming = read(&mut ws) => {
                let data = incoming?;
                last_rx = tokio::time::Instant::now();
                if let Some(err) = data.get("error") {
                    let code = number(&err["http_code"]);
                    let grpc = number(&err["grpc_code"]);
                    if code == 401 || code == 403 || grpc == 16 || grpc == 7 {
                        return Err("Требуется повторный вход".into());
                    }
                    let details = err.get("details").or_else(|| err.get("extra_headers")).unwrap_or(&Value::Null);
                    if details["ynison-error-code"].as_str() == Some("400090001") {
                        return Err("Устройство вытеснено; выключите и включите Ynison".into());
                    }
                    if let Some(schedule) = details["ynison-backoff-millis"].as_str() {
                        let parsed: Vec<u64> = schedule.split(':').take(32).filter_map(|s| s.parse::<u64>().ok())
                            .map(|ms| ms.clamp(1000, 86400000)).collect();
                        if !parsed.is_empty() { *backoff = parsed; }
                    }
                    let away = number(&details["ynison-go-away-for-seconds"]);
                    if away > 0 { return Ok(away); }
                    return Err("Ошибка сервера Ynison".into());
                }
                if !registered && data.as_object().is_some_and(|v| !v.is_empty()) {
                    registered = true;
                    event(Event::Status("Подключено".into()));
                }
                let was_active = active;
                if let Some(owner) = data.get("active_device_id_optional") {
                    active = owner.as_str() == Some(id);
                } else if data["player_state"]["status"]["version"]["device_id"].as_str()
                    == Some("server-paused-on-active-device-disconnecting") {
                    active = false;
                }
                let mut changed = active != was_active;
                if let Some(ps) = data["player_state"].as_object() {
                    for (key, value) in ps {
                        if value != &state["player_state"][key] {
                            if value["version"]["device_id"].as_str() != Some(id) { changed = true; }
                            if !state["player_state"].is_object() { state["player_state"] = json!({}); }
                            state["player_state"][key] = value.clone();
                        }
                    }
                }
                if let Some(devices) = data["devices"].as_array() {
                    state["devices"] = json!(devices);
                    let titles: Vec<&str> = devices.iter().filter_map(|d| d["info"]["title"].as_str()).collect();
                    event(Event::Devices(titles.join(" · ")));
                    if let Some(d) = devices.iter().find(|d| d["info"]["device_id"].as_str() == Some(id)) {
                        if let Some(v) = d["volume_info"]["volume"].as_f64().filter(|v| v.is_finite()) {
                            if (v as f32 - sent_volume).abs() > 0.001 {
                                sent_volume = v as f32;
                                state["remote_volume"] = json!(v);
                            }
                        }
                    }
                }
                if changed || state.get("remote_volume").is_some() { event(Event::State { state: state.clone(), active, changed }); }
                if let Some(map) = state.as_object_mut() { map.remove("remote_volume"); }
            }
            result = local.changed() => {
                if result.is_err() { return Ok(300); }
                let snapshot = local.borrow_and_update().clone();
                if (snapshot.volume - sent_volume).abs() > 0.001 {
                    sent_volume = snapshot.volume;
                    send(&mut ws, envelope(json!({"update_volume_info": {
                        "device_id": id, "volume_info": {"volume": snapshot.volume, "version": version(id)}
                    }}), id)).await?;
                }
                if snapshot.claim && !active && snapshot.now.is_some() {
                    send(&mut ws, envelope(json!({"update_active_device": {"device_id_optional": id}}), id)).await?;
                    active = true;
                    event(Event::State { state: json!({}), active: true, changed: false });
                }
                if active {
                    if let Some(now) = snapshot.now {
                        let ps = player_state(id, &now, &state["player_state"]["player_queue"]);
                        send(&mut ws, envelope(json!({"update_player_state": {"player_state": ps}}), id)).await?;
                    } else if state["player_state"]["status"].is_object() {
                        let mut status = state["player_state"]["status"].clone();
                        status["paused"] = json!(true);
                        status["version"] = version(id);
                        send(&mut ws, envelope(json!({"update_playing_status": {"playing_status": status}}), id)).await?;
                    }
                }
            }
            _ = ping.tick() => {
                if last_rx.elapsed() > heartbeat * 3 { return Err("Таймаут heartbeat".into()); }
                tokio::time::timeout(IO_TIMEOUT, ws.send(Message::Ping(Vec::new().into()))).await
                    .map_err(|_| "Таймаут heartbeat")?.map_err(|_| "Соединение прервано")?;
            }
        }
    }
}

pub(super) fn valid_host(host: &str) -> bool {
    let name = host.split(':').next().unwrap_or_default();
    (name.ends_with(".yandex.net") || name.ends_with(".yandex.ru"))
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b))
}

fn player_state(id: &str, now: &Now, retained: &Value) -> Value {
    let list: Vec<Value> = now.tracks.iter().map(|t| json!({
        "playable_id": t.id, "album_id_optional": t.album_id, "title": t.title,
        "playable_type": "TRACK", "from": now.from
    })).collect();
    let same = retained["playable_list"].as_array().is_some_and(|items|
        items.len() == list.len() && items.iter().zip(&list).all(|(a,b)| a["playable_id"] == b["playable_id"]));
    let mut queue = if same { retained.clone() } else { json!({
        "entity_id": now.entity_id, "entity_type": now.entity_type, "playable_list": list,
        "options": {"repeat_mode": "NONE"}, "entity_context": "BASED_ON_ENTITY_BY_DEFAULT",
        "from_optional": now.from
    }) };
    queue["current_playable_index"] = json!(now.index);
    queue["options"]["repeat_mode"] = json!(now.repeat);
    queue["version"] = version(id);
    json!({"player_queue": queue, "status": {
        "paused": now.paused, "progress_ms": now.progress_ms.to_string(),
        "duration_ms": now.duration_ms.to_string(), "playback_speed": 1,
        "version": version(id)
    }})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> Now {
        Now {
            entity_type: "VARIOUS", entity_id: String::new(), from: "test".into(),
            tracks: vec![Playable { id: "1".into(), album_id: "2".into(), title: "Track".into() }],
            index: 0, progress_ms: 1234, duration_ms: 10000, paused: true,
            session: String::new(), repeat: "ALL",
        }
    }
    #[test]
    fn snapshot_uses_protobuf_strings_and_preserves_queue_context() {
        let now = snapshot();
        let mut previous = player_state("pc", &now, &Value::Null)["player_queue"].clone();
        previous["extension"] = json!("retained");
        let state = player_state("pc", &now, &previous);
        assert_eq!(state["player_queue"]["extension"], "retained");
        assert_eq!(state["player_queue"]["options"]["repeat_mode"], "ALL");
        assert_eq!(state["status"]["progress_ms"], "1234");
        assert!(state["status"]["version"]["version"].is_string());
    }
    #[test]
    fn versions_are_monotonic_even_within_a_millisecond() {
        let a = number(&version("pc")["version"]);
        let b = number(&version("pc")["version"]);
        assert!(b > a);
    }
    #[test]
    fn watch_coalesces_without_queue_growth() {
        let (tx, rx) = watch::channel(0);
        for i in 0..10000 { tx.send_replace(i); }
        assert_eq!(*rx.borrow(), 9999);
    }
    #[test]
    fn shutdown_cancels_connect_and_joins_worker() {
        let mut service = Service::start("invalid\nheader".into(), "Test".into(), 0.5, || {});
        let worker = service.thread.take().unwrap();
        let start = std::time::Instant::now();
        drop(service);
        worker.join().unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn websocket_ping_and_state_are_read_without_polling() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                ws.send(Message::Ping(vec![1].into())).await.unwrap();
                ws.send(Message::text(r#"{"active_device_id_optional":"pc"}"#)).await.unwrap();
            });
            let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{address}")).await.unwrap();
            assert_eq!(read(&mut client).await.unwrap(), json!({}));
            assert_eq!(read(&mut client).await.unwrap()["active_device_id_optional"], "pc");
            server.await.unwrap();
        });
    }
    #[test]
    fn visible_player_registration() {
        let msg = hello("pc", "My PC", 0.6);
        let d = &msg["update_full_state"]["device"];
        assert_eq!(d["capabilities"]["can_be_player"], true);
        assert_eq!(d["is_shadow"], false);
        assert_eq!(d["info"]["title"], "My PC");
        assert!(msg["player_action_timestamp_ms"].is_string());
    }
    #[test]
    fn redirect_cannot_exfiltrate_token() {
        assert!(valid_host("ynison.music.yandex.net"));
        assert!(valid_host("test.music.yandex.ru:443"));
        for bad in ["evil.org", "music.yandex.ru.evil.org", "evil.org/music.yandex.ru", "a@x.music.yandex.ru"] {
            assert!(!valid_host(bad), "{bad}");
        }
    }
}
