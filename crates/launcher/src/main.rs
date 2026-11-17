#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    process::Command,
};

#[derive(Debug, Default, Serialize, Deserialize)]
struct LauncherConfig {
    nickname: String,
}

struct LauncherApp {
    nickname: String,
    message: String,
    message_is_error: bool,
}

impl Default for LauncherApp {
    fn default() -> Self {
        let config = load_config();
        Self {
            nickname: config.nickname,
            message: "Локальный офлайн-профиль. Авторизация не требуется.".into(),
            message_is_error: false,
        }
    }
}

impl LauncherApp {
    fn save(&self) -> Result<(), String> {
        let path = config_path()?;
        let parent = path.parent().ok_or("Не удалось определить папку настроек")?;
        fs::create_dir_all(parent).map_err(|e| format!("Не удалось создать папку настроек: {e}"))?;
        let config = LauncherConfig { nickname: self.nickname.trim().to_owned() };
        let data = serde_json::to_vec_pretty(&config)
            .map_err(|e| format!("Не удалось сохранить настройки: {e}"))?;
        fs::write(path, data).map_err(|e| format!("Не удалось записать настройки: {e}"))
    }

    fn launch(&mut self) -> bool {
        let nickname = self.nickname.trim();
        if !valid_nickname(nickname) {
            self.message = "Ник: 3–16 символов, только латинские буквы, цифры и _.".into();
            self.message_is_error = true;
            return false;
        }

        if let Err(error) = self.save() {
            self.message = error;
            self.message_is_error = true;
            return false;
        }

        let Some(client) = find_client_executable() else {
            self.message = "Не найден rust-edition.exe. Положи его рядом с лаунчером или собери проект командой cargo build --release -p rust-edition-client.".into();
            self.message_is_error = true;
            return false;
        };

        let working_dir = client.parent().map(PathBuf::from);
        let mut command = Command::new(client);
        command.arg("--window").arg("--username").arg(nickname);
        if let Some(directory) = working_dir {
            command.current_dir(directory);
        }

        match command.spawn() {
            Ok(_) => {
                self.message = format!("Запускаю Rust Edition под ником {nickname}.");
                self.message_is_error = false;
                true
            }
            Err(error) => {
                self.message = format!("Не удалось запустить игру: {error}");
                self.message_is_error = true;
                false
            }
        }
    }
}

impl eframe::App for LauncherApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::from_rgb(16, 13, 25)).inner_margin(egui::Margin::same(26.0)))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(12.0);
                    ui.heading(egui::RichText::new("RUST EDITION").size(30.0).strong().color(egui::Color32::from_rgb(232, 220, 255)));
                    ui.label(egui::RichText::new("Minecraft 1.8.9 • простой лаунчер").color(egui::Color32::from_rgb(160, 148, 180)));
                    ui.add_space(28.0);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new("ОФЛАЙН-НИК").strong().color(egui::Color32::from_rgb(198, 183, 220)));
                    });
                    let edit = egui::TextEdit::singleline(&mut self.nickname)
                        .hint_text("Введите ник")
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(12.0, 11.0));
                    let edit_response = ui.add(edit);
                    let enter_pressed = edit_response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Ник сохраняется на этом компьютере. Аккаунт Microsoft не нужен.")
                        .size(12.0).color(egui::Color32::from_rgb(145, 135, 160)));
                    ui.add_space(24.0);
                    let play = egui::Button::new(egui::RichText::new("▶   ИГРАТЬ").size(18.0).strong())
                        .fill(egui::Color32::from_rgb(116, 74, 190))
                        .min_size(egui::vec2(ui.available_width(), 48.0));
                    if ui.add(play).clicked() || enter_pressed {
                        if self.launch() { _frame.close(); }
                    }
                    ui.add_space(14.0);
                    let color = if self.message_is_error { egui::Color32::from_rgb(255, 130, 130) } else { egui::Color32::from_rgb(155, 220, 175) };
                    ui.label(egui::RichText::new(&self.message).size(12.0).color(color));
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Ранний прототип • мультиплеер пока не реализован")
                        .size(11.0).color(egui::Color32::from_rgb(110, 102, 125)));
                });
            });
    }
}

fn valid_nickname(name: &str) -> bool {
    (3..=16).contains(&name.len())
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn config_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from)
        .ok_or("Переменная APPDATA не найдена")?;
    #[cfg(not(target_os = "windows"))]
    let base = std::env::var_os("HOME").map(PathBuf::from)
        .ok_or("Не удалось определить домашнюю папку")?
        .join(".config");
    Ok(base.join("RustEdition").join("launcher.json"))
}

fn load_config() -> LauncherConfig {
    config_path().ok()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn find_client_executable() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let launcher_dir = exe.parent()?;
    let exe_name = if cfg!(windows) { "rust-edition.exe" } else { "rust-edition" };
    let mut candidates = vec![
        launcher_dir.join(exe_name),
        launcher_dir.join("game").join(exe_name),
        launcher_dir.join("..").join("..").join("target").join("release").join(exe_name),
        std::env::current_dir().ok()?.join("target").join("release").join(exe_name),
    ];
    candidates.retain(|path| path.is_file());
    candidates.into_iter().map(|path| fs::canonicalize(&path).unwrap_or(path)).find(|path| path.is_file())
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([480.0, 390.0])
            .with_min_inner_size([420.0, 350.0])
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "Rust Edition Launcher",
        options,
        Box::new(|_creation_context| Box::<LauncherApp>::default()),
    )
}

#[cfg(test)]
mod tests {
    use super::valid_nickname;

    #[test]
    fn accepts_minecraft_style_offline_names() {
        assert!(valid_nickname("Player_123"));
        assert!(valid_nickname("abc"));
    }

    #[test]
    fn rejects_invalid_offline_names() {
        assert!(!valid_nickname("ab"));
        assert!(!valid_nickname("name with spaces"));
        assert!(!valid_nickname("имя"));
        assert!(!valid_nickname("this_name_is_way_too_long"));
    }
}
