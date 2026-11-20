// SPDX-License-Identifier: GPL-3.0-or-later
use arboard::{Clipboard, ImageData};
use enigo::{
    Direction::{Click, Press, Release},
    Enigo, Key, Keyboard, Settings,
};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, sync::Mutex, thread, time::Duration};
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
        eprintln!("paste: restore failed: {e}");
    }
}

/// Pega `text` en la app activa vía portapapeles y restaura el portapapeles previo.
fn paste(text: String) {
    thread::spawn(move || {
        // Esperamos a que el usuario suelte la combinación, si no se mezclan los modificadores.
        thread::sleep(Duration::from_millis(350));
        eprintln!("paste: start");
        let Ok(mut cb) = Clipboard::new() else {
            return eprintln!("paste: clipboard open failed");
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
            Err(e) => return eprintln!("paste: enigo failed: {e}"),
        };
        eprintln!("paste: enigo ok, sending keys");
        #[cfg(target_os = "macos")]
        let m = Key::Meta;
        #[cfg(not(target_os = "macos"))]
        let m = Key::Control;
        let _ = en.key(m, Press);
        let _ = en.key(Key::Unicode('v'), Click);
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

fn main() {
    tauri::Builder::default()
        .manage(State::default())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
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
                    eprintln!("hotkey {sc:?} matched={}", text.is_some());
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
            set_autostart
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();
            let _ = apply(&handle, &load(&handle));

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
        .run(tauri::generate_context!())
        .expect("error al iniciar octopus");
}

#[cfg(test)]
mod tests {
    use super::*;
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
