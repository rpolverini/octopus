// SPDX-License-Identifier: GPL-3.0-or-later
// En release, Windows no abre la ventana de consola; en debug se mantiene para ver los eprintln!.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use arboard::{Clipboard, ImageData};
use enigo::{
    Direction::{Click, Press, Release},
    Enigo, Key, Keyboard, Settings,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    io::Write,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
    },
    thread,
    time::Duration,
};
use tauri::{
    AppHandle, Manager, WindowEvent,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

// Hardcoded: abre la configuración. No se puede asignar a un snippet.
// Ctrl+Alt en Windows/Linux es AltGr (rompe @ en teclados latam/es), por eso lleva Shift.
#[cfg(target_os = "macos")]
const OPEN_COMBO: &str = "Ctrl+Alt+O";
#[cfg(not(target_os = "macos"))]
const OPEN_COMBO: &str = "Ctrl+Shift+Alt+O";

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    combo: String,
    text: String,
    desc: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Profile {
    name: String,
    entries: Vec<Entry>,
}

/// Solo los hotkeys de la plantilla `active` están registrados.
#[derive(Clone, Serialize, Deserialize)]
struct Config {
    active: usize,
    profiles: Vec<Profile>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            active: 0,
            profiles: vec![Profile {
                name: "Default".into(),
                entries: vec![],
            }],
        }
    }
}

impl Config {
    fn fixed(mut self) -> Self {
        if self.profiles.is_empty() {
            return Config::default();
        }
        self.active = self.active.min(self.profiles.len() - 1);
        self
    }
}

/// Acepta el formato nuevo y el viejo (lista plana de entradas, que pasa a ser "Default").
fn parse_config(s: &str) -> Config {
    if let Ok(c) = serde_json::from_str::<Config>(s) {
        return c.fixed();
    }
    match serde_json::from_str::<Vec<Entry>>(s) {
        Ok(entries) => Config {
            active: 0,
            profiles: vec![Profile {
                name: "Default".into(),
                entries,
            }],
        },
        Err(_) => Config::default(),
    }
}

#[derive(Default)]
struct State(Mutex<Vec<(Shortcut, String)>>);

fn config_path(app: &AppHandle) -> PathBuf {
    let dir = app.path().app_config_dir().expect("config dir");
    fs::create_dir_all(&dir).ok();
    dir.join("octopus.json")
}

fn load(app: &AppHandle) -> Config {
    fs::read_to_string(config_path(app))
        .map(|s| parse_config(&s))
        .unwrap_or_default()
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Copia del portapapeles para poder devolverlo después de pegar.
enum Saved {
    Files(Vec<PathBuf>),
    Image(ImageData<'static>),
    Html(String, Option<String>),
    Text(String),
    /// Algo que no sabemos copiar: mejor vaciar que dejar el texto pegado.
    Other,
}

fn save_clipboard(cb: &mut Clipboard) -> Saved {
    if let Ok(f) = cb.get().file_list() {
        return Saved::Files(f);
    }
    if let Ok(i) = cb.get().image() {
        return Saved::Image(i);
    }
    if let Ok(h) = cb.get().html() {
        return Saved::Html(h, cb.get_text().ok());
    }
    match cb.get_text() {
        Ok(t) => Saved::Text(t),
        Err(_) => Saved::Other,
    }
}

fn restore_clipboard(cb: &mut Clipboard, saved: Saved) {
    let r = match saved {
        Saved::Files(f) => cb.set().file_list(&f),
        Saved::Image(i) => cb.set().image(i),
        Saved::Html(h, alt) => cb.set().html(h, alt),
        Saved::Text(t) => cb.set_text(t),
        Saved::Other => cb.clear(),
    };
    if let Err(e) = r {
        dlog(&format!("paste: restore failed: {e}"));
    }
}

/// Log opcional (pestaña Funciones adicionales). Vive junto a la config de cada instalación.
static DEBUG: AtomicBool = AtomicBool::new(false);
static LOG: OnceLock<PathBuf> = OnceLock::new();

static LAST_PRUNE: AtomicU64 = AtomicU64::new(0);

/// Deja solo las líneas de los últimos 2 días (cada línea empieza con `[unix]`).
fn prune_log(p: &PathBuf, now: u64) {
    let Ok(s) = fs::read_to_string(p) else { return };
    let min = now.saturating_sub(2 * 24 * 3600);
    let kept: String = s
        .lines()
        .filter(|l| {
            l.strip_prefix('[')
                .and_then(|r| r.split(']').next())
                .and_then(|t| t.parse::<u64>().ok())
                .is_none_or(|t| t >= min)
        })
        .map(|l| format!("{l}\n"))
        .collect();
    if kept.len() != s.len() {
        let _ = fs::write(p, kept);
    }
}

fn dlog(msg: &str) {
    eprintln!("{msg}");
    if !DEBUG.load(Relaxed) {
        return;
    }
    let Some(p) = LOG.get() else { return };
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // al arrancar y luego como mucho una vez por hora
    if t.saturating_sub(LAST_PRUNE.load(Relaxed)) > 3600 {
        LAST_PRUNE.store(t, Relaxed);
        prune_log(p, t);
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "[{t}] {msg}");
    }
}

fn debug_flag() -> PathBuf {
    LOG.get().unwrap().with_file_name("debug-log-off")
}

#[tauri::command]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
fn get_debug_log() -> (bool, String) {
    (DEBUG.load(Relaxed), LOG.get().unwrap().display().to_string())
}

#[tauri::command]
fn set_debug_log(on: bool) -> Result<(), String> {
    // activado por defecto: el archivo marca que el usuario lo apagó
    if on {
        let _ = fs::remove_file(debug_flag());
    } else {
        fs::write(debug_flag(), "").map_err(|e| e.to_string())?;
    }
    DEBUG.store(on, Relaxed);
    if on {
        dlog(&format!("log activado, octopus {}", env!("CARGO_PKG_VERSION")));
    }
    Ok(())
}

/// Abre la carpeta del log con el explorador del sistema.
#[tauri::command]
fn reveal_log() -> Result<(), String> {
    let p = LOG.get().unwrap();
    let dir = p.parent().unwrap();
    let _ = fs::create_dir_all(dir);
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let r = std::process::Command::new("xdg-open").arg(dir).spawn();
    r.map(|_| ()).map_err(|e| e.to_string())
}

/// Pega `text` en la app activa vía portapapeles y restaura el portapapeles previo.
fn paste(text: String) {
    thread::spawn(move || {
        // Esperamos a que el usuario suelte la combinación, si no se mezclan los modificadores.
        thread::sleep(Duration::from_millis(350));
        dlog("paste: start");
        let Ok(mut cb) = Clipboard::new() else {
            return dlog("paste: clipboard open failed");
        };
        let saved = save_clipboard(&mut cb);
        if cb.set_text(text).is_err() {
            return;
        }
        let mut en = match Enigo::new(&Settings {
            open_prompt_to_get_permissions: true,
            ..Settings::default()
        }) {
            Ok(e) => e,
            Err(e) => return dlog(&format!("paste: enigo failed: {e}")),
        };
        dlog("paste: enigo ok, sending keys");
        #[cfg(target_os = "macos")]
        let m = Key::Meta;
        #[cfg(not(target_os = "macos"))]
        let m = Key::Control;
        let _ = en.key(m, Press);
        // macOS: Key::Unicode consulta TSM fuera del hilo principal y el sistema aborta el proceso.
        // 9 = kVK_ANSI_V (no depende de la distribución de teclado).
        #[cfg(target_os = "macos")]
        let v = Key::Other(9);
        #[cfg(not(target_os = "macos"))]
        let v = Key::Unicode('v');
        let _ = en.key(v, Click);
        let _ = en.key(m, Release);
        thread::sleep(Duration::from_millis(250));
        restore_clipboard(&mut cb, saved);
    });
}

/// Registra el hotkey de apertura + los de la lista. Devuelve los errores por combo.
fn apply(app: &AppHandle, cfg: &Config) -> Result<(), String> {
    let entries = &cfg.profiles[cfg.active].entries;
    let gs = app.global_shortcut();
    gs.unregister_all().map_err(|e| e.to_string())?;
    let open: Shortcut = OPEN_COMBO.parse().unwrap();
    let _ = gs.register(open);
    let mut ok = vec![];
    let mut errs = vec![];
    for e in entries {
        match e.combo.parse::<Shortcut>() {
            Ok(s) if s == open => errs.push(format!(
                "{}: reservada para abrir la configuración",
                e.combo
            )),
            Ok(s) if ok.iter().any(|(o, _): &(Shortcut, String)| *o == s) => {
                errs.push(format!("{}: repetida", e.combo))
            }
            Ok(s) => match gs.register(s) {
                Ok(_) => ok.push((s, e.text.clone())),
                Err(err) => errs.push(format!("{}: {}", e.combo, err)),
            },
            Err(_) => errs.push(format!("{}: combinación inválida", e.combo)),
        }
    }
    *app.state::<State>().0.lock().unwrap() = ok;
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}

#[tauri::command]
fn open_combo() -> &'static str {
    OPEN_COMBO
}

#[tauri::command]
fn get_config(app: AppHandle) -> Config {
    load(&app)
}

#[tauri::command]
fn save_config(app: AppHandle, config: Config) -> Result<(), String> {
    let config = config.fixed();
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(config_path(&app), json).map_err(|e| e.to_string())?;
    apply(&app, &config)
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: AppHandle, on: bool) -> Result<(), String> {
    let a = app.autolaunch();
    if on { a.enable() } else { a.disable() }.map_err(|e| e.to_string())
}

/// Invertir el scroll solo del mouse con rueda (no del trackpad). Se guarda como un archivo vacío.
static INVERT_MOUSE: AtomicBool = AtomicBool::new(false);

fn invert_flag(app: &AppHandle) -> PathBuf {
    config_path(app).with_file_name("invert-mouse-scroll")
}

/// Arranca (una sola vez) el hilo con el event tap. Falla si falta el permiso de Accesibilidad.
#[cfg(target_os = "macos")]
fn start_scroll_tap() -> Result<(), String> {
    use core_graphics::event::{
        CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
        CallbackResult, EventField as F,
    };
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.load(Relaxed) {
        return Ok(());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let tx2 = tx.clone();
    thread::spawn(move || {
        let r = CGEventTap::with_enabled(
            CGEventTapLocation::Session,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
            vec![CGEventType::ScrollWheel],
            |_, _, ev| {
                // El trackpad (y el Magic Mouse) mandan scroll "continuo"; la rueda USB no.
                if INVERT_MOUSE.load(Relaxed)
                    && ev.get_integer_value_field(F::SCROLL_WHEEL_EVENT_IS_CONTINUOUS) == 0
                {
                    // Leer todo antes de escribir: al escribir un campo macOS recalcula los otros.
                    let i = F::SCROLL_WHEEL_EVENT_DELTA_AXIS_1;
                    let (fx, pt) = (
                        F::SCROLL_WHEEL_EVENT_FIXED_POINT_DELTA_AXIS_1,
                        F::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_1,
                    );
                    let (vi, vf, vp) = (
                        ev.get_integer_value_field(i),
                        ev.get_double_value_field(fx),
                        ev.get_double_value_field(pt),
                    );
                    ev.set_integer_value_field(i, -vi);
                    ev.set_double_value_field(fx, -vf);
                    ev.set_double_value_field(pt, -vp);
                }
                CallbackResult::Keep
            },
            || {
                let _ = tx.send(Ok(()));
                core_foundation::runloop::CFRunLoop::run_current();
            },
        );
        if r.is_err() {
            let _ = tx2.send(Err(
                "Falta el permiso de Accesibilidad: activalo para octopus en Ajustes del Sistema → Privacidad y seguridad → Accesibilidad, y volvé a probar.".to_string(),
            ));
        }
    });
    let r = rx.recv().map_err(|e| e.to_string())?;
    STARTED.store(r.is_ok(), Relaxed);
    r
}

#[cfg(not(target_os = "macos"))]
fn start_scroll_tap() -> Result<(), String> {
    Err("Solo disponible en macOS".into())
}

#[tauri::command]
fn get_invert_mouse(app: AppHandle) -> bool {
    invert_flag(&app).exists()
}

#[tauri::command]
fn set_invert_mouse(app: AppHandle, on: bool) -> Result<(), String> {
    if on {
        start_scroll_tap()?;
        fs::write(invert_flag(&app), "").map_err(|e| e.to_string())?;
    } else {
        let _ = fs::remove_file(invert_flag(&app));
    }
    INVERT_MOUSE.store(on, Relaxed);
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .manage(State::default())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, sc, ev| {
                    if ev.state() != ShortcutState::Pressed {
                        return;
                    }
                    if *sc == OPEN_COMBO.parse::<Shortcut>().unwrap() {
                        return show_window(app);
                    }
                    let text = app
                        .state::<State>()
                        .0
                        .lock()
                        .unwrap()
                        .iter()
                        .find(|(s, _)| s == sc)
                        .map(|(_, t)| t.clone());
                    dlog(&format!("hotkey {sc:?} matched={}", text.is_some()));
                    if let Some(t) = text {
                        paste(t);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            open_combo,
            get_config,
            save_config,
            get_autostart,
            set_autostart,
            get_invert_mouse,
            set_invert_mouse,
            version,
            get_debug_log,
            set_debug_log,
            reveal_log
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();
            let _ = LOG.set(config_path(&handle).with_file_name("octopus.log"));
            DEBUG.store(!debug_flag().exists(), Relaxed);
            let prev = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |i| {
                dlog(&format!("PANIC: {i}"));
                prev(i);
            }));
            dlog("arranque");
            // Abierta si la lanza el usuario (si no, parece que no arrancó); escondida si la lanza el login.
            if !std::env::args().any(|a| a == "--autostart") {
                show_window(&handle);
            }
            let _ = apply(&handle, &load(&handle));
            if invert_flag(&handle).exists() {
                // sin permiso falla en silencio; la pestaña lo muestra al reintentar
                INVERT_MOUSE.store(start_scroll_tap().is_ok(), Relaxed);
            }

            let menu = Menu::with_items(
                app,
                &[
                    &MenuItem::with_id(app, "open", "Configuración", true, None::<&str>)?,
                    &MenuItem::with_id(app, "quit", "Salir", true, None::<&str>)?,
                ],
            )?;
            TrayIconBuilder::new()
                .icon(tauri::include_image!("icons/tray.png"))
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| match ev.id.as_ref() {
                    "open" => show_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|t, ev| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = ev
                    {
                        show_window(t.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|w, ev| {
            // cerrar = esconder; la app vive en la bandeja
            if let WindowEvent::CloseRequested { api, .. } = ev {
                api.prevent_close();
                let _ = w.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("error al iniciar octopus")
        .run(|_app, _ev| {
            // macOS: abrir octopus de nuevo desde Finder/Launchpad muestra la configuración
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _ev {
                show_window(_app);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prune_keeps_two_days() {
        let p = std::env::temp_dir().join("octopus-prune-test.log");
        fs::write(&p, "[100] viejo\n[900000] nuevo\nsin fecha\n").unwrap();
        prune_log(&p, 1_000_000);
        assert_eq!(fs::read_to_string(&p).unwrap(), "[900000] nuevo\nsin fecha\n");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn ui_combos_parse() {
        // formatos que genera la UI: Key*/Digit* recortados, y codes tal cual
        for c in [
            OPEN_COMBO,
            "Ctrl+Alt+E",
            "Ctrl+Alt+Shift+5",
            "Ctrl+Alt+Space",
            "Ctrl+Cmd+F1",
            "Alt+ArrowUp",
            "Ctrl+Alt+Minus",
        ] {
            assert!(c.parse::<Shortcut>().is_ok(), "{c}");
        }
    }

    #[test]
    fn config_migrates_legacy_and_clamps() {
        let legacy = r#"[{"combo":"Ctrl+Alt+E","text":"a","desc":"d"}]"#;
        let c = parse_config(legacy);
        assert_eq!(
            (c.active, c.profiles.len(), c.profiles[0].entries.len()),
            (0, 1, 1)
        );
        let bad = r#"{"active":9,"profiles":[{"name":"x","entries":[]}]}"#;
        assert_eq!(parse_config(bad).active, 0);
        assert_eq!(parse_config("basura").profiles.len(), 1);
    }
}
