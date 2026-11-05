use eframe::egui;
use egui::{Color32, FontId, RichText, Stroke, Vec2};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::config::{AppConfig, InjectionMode};
use crate::injector::SharedState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Injector,
    Compatibility,
    Guide,
    About,
}

pub struct MouseInjectorApp {
    state: Arc<SharedState>,
    config_path: String,
    current_tab: Tab,
    last_saved_sens: u32,
    last_saved_invert: bool,
    last_saved_mode: InjectionMode,
}

impl MouseInjectorApp {
    pub fn new(
        _cc: &eframe::CreationContext<'_>,
        state: Arc<SharedState>,
        config_path: String,
    ) -> Self {
        let initial_mode = state.get_mode();
        let initial_sens = state.sensitivity.load(Ordering::Relaxed);
        let initial_invert = state.invert_pitch.load(Ordering::Relaxed);

        Self {
            state,
            config_path,
            current_tab: Tab::Injector,
            last_saved_sens: initial_sens,
            last_saved_invert: initial_invert,
            last_saved_mode: initial_mode,
        }
    }

    fn check_and_save_config(&mut self) {
        let cur_mode = self.state.get_mode();
        let cur_sens = self.state.sensitivity.load(Ordering::Relaxed);
        let cur_invert = self.state.invert_pitch.load(Ordering::Relaxed);

        if cur_mode != self.last_saved_mode
            || cur_sens != self.last_saved_sens
            || cur_invert != self.last_saved_invert
        {
            self.last_saved_mode = cur_mode;
            self.last_saved_sens = cur_sens;
            self.last_saved_invert = cur_invert;

            let cfg = AppConfig {
                mode: cur_mode,
                sensitivity: cur_sens,
                invert_pitch: cur_invert,
            };
            cfg.save(&self.config_path);
        }
    }
}

impl eframe::App for MouseInjectorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        self.check_and_save_config();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);

            ui.vertical_centered(|ui| {
                ui.add_space(2.0);
                ui.heading(
                    RichText::new("PCSX2 Mouse Injector")
                        .size(20.0)
                        .strong()
                        .color(Color32::from_rgb(225, 235, 245)),
                );
            });

            ui.add_space(2.0);

            self.render_navbar(ui);

            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    match self.current_tab {
                        Tab::Injector => self.render_injector_tab(ui),
                        Tab::Compatibility => self.render_compatibility_tab(ui),
                        Tab::Guide => self.render_guide_tab(ui),
                        Tab::About => self.render_about_tab(ui),
                    }
                });
        });
    }
}

impl MouseInjectorApp {
    fn render_navbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let tabs = [
                (Tab::Injector, "Injector"),
                (Tab::Compatibility, "Compatibility"),
                (Tab::Guide, "Guide"),
                (Tab::About, "About"),
            ];

            let count = tabs.len() as f32;
            let spacing = (count - 1.0) * 6.0;
            let btn_width = ((ui.available_width() - spacing) / count).max(50.0);
            let btn_size = Vec2::new(btn_width, 28.0);

            for (tab, label) in tabs {
                let is_selected = self.current_tab == tab;
                let btn = egui::Button::new(
                    RichText::new(label)
                        .size(12.5)
                        .strong()
                        .color(if is_selected {
                            Color32::WHITE
                        } else {
                            Color32::from_rgb(170, 180, 195)
                        }),
                )
                .fill(if is_selected {
                    Color32::from_rgb(45, 95, 170)
                } else {
                    Color32::from_rgb(28, 33, 42)
                })
                .min_size(btn_size);

                if ui.add(btn).clicked() {
                    self.current_tab = tab;
                }
            }
        });
    }

    fn render_injector_tab(&mut self, ui: &mut egui::Ui) {
        let connected = self.state.pcsx2_connected.load(Ordering::Relaxed);
        let pid = self.state.pcsx2_pid.load(Ordering::Relaxed);
        let emu_name = self.state.pcsx2_name.read().unwrap().clone();
        let focused = self.state.pcsx2_focused.load(Ordering::Relaxed);
        let game_name = self.state.active_game_name.read().unwrap().clone();
        let mode = self.state.get_mode();

        egui::Frame::none()
            .fill(Color32::from_rgb(24, 28, 36))
            .stroke(Stroke::new(1.0, Color32::from_rgb(42, 48, 60)))
            .rounding(8.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.label(
                    RichText::new("SYSTEM STATUS")
                        .font(FontId::proportional(11.0))
                        .color(Color32::from_rgb(120, 140, 165))
                        .strong(),
                );
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Emulator:").size(13.0).color(Color32::LIGHT_GRAY));
                    if connected {
                        let text = format!("{} (PID: {})", emu_name, pid);
                        ui.colored_label(Color32::from_rgb(70, 210, 120), text);
                    } else {
                        ui.colored_label(Color32::from_rgb(240, 180, 50), "Searching for pcsx2-qt.exe...");
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Window Focus:").size(13.0).color(Color32::LIGHT_GRAY));
                    if connected {
                        if focused {
                            ui.colored_label(Color32::from_rgb(80, 190, 255), "PCSX2 in Foreground");
                        } else {
                            ui.colored_label(Color32::from_rgb(160, 160, 170), "Background / Unfocused");
                        }
                    } else {
                        ui.colored_label(Color32::GRAY, "-");
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Game Detected:").size(13.0).color(Color32::LIGHT_GRAY));
                    if let Some(game) = game_name {
                        ui.colored_label(Color32::from_rgb(80, 220, 140), game);
                    } else if connected {
                        ui.colored_label(Color32::from_rgb(230, 170, 60), "Waiting for supported game in EE RAM...");
                    } else {
                        ui.colored_label(Color32::GRAY, "-");
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Config File:").size(13.0).color(Color32::LIGHT_GRAY));
                    let avail_w = ui.available_width().max(60.0);
                    ui.add_sized(
                        [avail_w, 18.0],
                        egui::Label::new(
                            RichText::new(&self.config_path)
                                .size(12.0)
                                .color(Color32::from_rgb(140, 180, 220)),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&self.config_path);
                });
            });

        ui.add_space(4.0);

        ui.label(
            RichText::new("INJECTION MODE")
                .font(FontId::proportional(11.0))
                .color(Color32::from_rgb(120, 140, 165))
                .strong(),
        );

        ui.horizontal(|ui| {
            let off_selected = mode == InjectionMode::Off;
            let auto_selected = mode == InjectionMode::Auto;

            let btn_size = Vec2::new((ui.available_width() - 10.0) / 2.0, 36.0);

            let off_btn = egui::Button::new(
                RichText::new("OFF")
                    .size(14.0)
                    .strong()
                    .color(if off_selected { Color32::WHITE } else { Color32::GRAY }),
            )
            .fill(if off_selected {
                Color32::from_rgb(180, 50, 50)
            } else {
                Color32::from_rgb(32, 36, 45)
            })
            .min_size(btn_size);

            let auto_btn = egui::Button::new(
                RichText::new("AUTO")
                    .size(14.0)
                    .strong()
                    .color(if auto_selected { Color32::BLACK } else { Color32::GRAY }),
            )
            .fill(if auto_selected {
                Color32::from_rgb(60, 210, 120)
            } else {
                Color32::from_rgb(32, 36, 45)
            })
            .min_size(btn_size);

            if ui.add(off_btn).clicked() {
                self.state.set_mode(InjectionMode::Off);
            }
            if ui.add(auto_btn).clicked() {
                self.state.set_mode(InjectionMode::Auto);
            }
        });

        ui.add_space(6.0);
        ui.separator();

        ui.label(
            RichText::new("SETTINGS")
                .font(FontId::proportional(11.0))
                .color(Color32::from_rgb(120, 140, 165))
                .strong(),
        );

        let mut sens = self.state.sensitivity.load(Ordering::Relaxed);
        let pct = sens * 5;
        ui.label(RichText::new(format!("Sensitivity: {}%", pct)).size(13.0));

        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(RichText::new("-").strong()).min_size(Vec2::new(22.0, 20.0)))
                .clicked()
            {
                sens = sens.saturating_sub(1).max(1);
                self.state.sensitivity.store(sens, Ordering::Relaxed);
            }

            if ui.add(egui::Slider::new(&mut sens, 1..=100)).changed() {
                self.state.sensitivity.store(sens, Ordering::Relaxed);
            }

            if ui
                .add(egui::Button::new(RichText::new("+").strong()).min_size(Vec2::new(22.0, 20.0)))
                .clicked()
            {
                sens = (sens + 1).min(100);
                self.state.sensitivity.store(sens, Ordering::Relaxed);
            }

            ui.label(
                RichText::new("units (20 = 100%)")
                    .size(12.0)
                    .color(Color32::from_rgb(160, 175, 190)),
            );
        });

        ui.add_space(2.0);

        let mut invert = self.state.invert_pitch.load(Ordering::Relaxed);
        if ui.checkbox(&mut invert, "Invert Vertical Look (Pitch)").changed() {
            self.state.invert_pitch.store(invert, Ordering::Relaxed);
        }
    }

    fn render_compatibility_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading(
            RichText::new("Compatible Games")
                .size(18.0)
                .strong()
                .color(Color32::WHITE),
        );

        ui.label(
            RichText::new("Supported game:")
                .size(12.0)
                .color(Color32::from_rgb(140, 160, 180)),
        );

        ui.add_space(8.0);

        egui::Frame::none()
            .fill(Color32::from_rgb(22, 26, 34))
            .stroke(Stroke::new(1.0, Color32::from_rgb(45, 110, 75)))
            .rounding(8.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.label(
                    RichText::new("Black (PS2)")
                        .size(16.0)
                        .strong()
                        .color(Color32::WHITE),
                );

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                let card_w = ui.available_width();
                let col1_w = 90.0;
                let spacing = 16.0;
                let col2_max = (card_w - col1_w - spacing).max(100.0);

                egui::Grid::new("black_compat_grid")
                    .num_columns(2)
                    .spacing([spacing, 8.0])
                    .max_col_width(col2_max)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Emulators:").strong());
                        ui.add(egui::Label::new("support all version pscx2 (32-bit / 64-bit)").wrap());
                        ui.end_row();

                        ui.label(RichText::new(""));
                        ui.vertical(|ui| {
                            ui.add(egui::Label::new("If the application does not work on a specific version, you can report it").wrap());
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                ui.label("via a GitHub ");
                                ui.hyperlink_to("issue", "https://github.com/lorem/pcsx2-mouse-injector/issues");
                                ui.label(".");
                            });
                        });
                        ui.end_row();

                        ui.label(RichText::new("Regions:").strong());
                        ui.add(egui::Label::new("NTSC-U (SLUS-21376) & PAL (SLES-53886)").wrap());
                        ui.end_row();
                    });
            });

        ui.add_space(12.0);

        egui::Frame::none()
            .fill(Color32::from_rgb(26, 30, 38))
            .stroke(Stroke::new(1.0, Color32::from_rgb(45, 52, 65)))
            .rounding(6.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(
                    egui::Label::new(
                        RichText::new("Coming soon for other games...")
                            .size(11.5)
                            .color(Color32::from_rgb(160, 175, 190)),
                    )
                    .wrap(),
                );
            });
    }

    fn render_guide_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading(
            RichText::new("User Guide")
                .size(18.0)
                .strong()
                .color(Color32::WHITE),
        );
        ui.add_space(4.0);

        egui::Frame::none()
            .fill(Color32::from_rgb(22, 26, 34))
            .stroke(Stroke::new(1.0, Color32::from_rgb(40, 46, 58)))
            .rounding(6.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("1. Injection Modes").strong().size(14.0));
                ui.add(egui::Label::new("• AUTO: Automatically locks mouse to PCSX2 window while focused; releases cursor on Alt+Tab.").wrap());
                ui.add(egui::Label::new("• OFF: Disables injection and allows mouse to move freely.").wrap());

                ui.add_space(8.0);
                ui.label(RichText::new("2. Sensitivity & Look").strong().size(14.0));
                ui.add(egui::Label::new("• Sensitivity: Controls camera turning speed (default 100% = 20 units).").wrap());
                ui.add(egui::Label::new("• Invert Pitch: Inverts vertical (up/down) look direction.").wrap());
            });
    }

    fn render_about_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading(
            RichText::new("About")
                .size(18.0)
                .strong()
                .color(Color32::WHITE),
        );
        ui.add_space(4.0);

        egui::Frame::none()
            .fill(Color32::from_rgb(22, 26, 34))
            .stroke(Stroke::new(1.0, Color32::from_rgb(40, 46, 58)))
            .rounding(6.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.label(
                    RichText::new("PCSX2 Mouse Injector")
                        .size(16.0)
                        .strong()
                        .color(Color32::from_rgb(80, 170, 240)),
                );
                ui.label(
                    RichText::new("Direct EE RAM camera injector for PCSX2")
                        .size(12.0)
                        .color(Color32::from_rgb(160, 175, 190)),
                );

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                let card_w = ui.available_width();
                let col1_w = 95.0;
                let spacing = 16.0;
                let col2_max = (card_w - col1_w - spacing).max(100.0);

                egui::Grid::new("about_grid")
                    .num_columns(2)
                    .spacing([spacing, 8.0])
                    .max_col_width(col2_max)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Version:").strong());
                        ui.add(egui::Label::new("1.0.0").wrap());
                        ui.end_row();

                        ui.label(RichText::new("Author:").strong());
                        ui.add(egui::Label::new("lorem").wrap());
                        ui.end_row();

                        ui.label(RichText::new("License:").strong());
                        ui.add(egui::Label::new("GPL-3.0 / MIT").wrap());
                        ui.end_row();

                        ui.label(RichText::new("Stack:").strong());
                        ui.add(egui::Label::new("Rust (egui + Win32 API)").wrap());
                        ui.end_row();

                        ui.label(RichText::new("Repository:").strong());
                        ui.add(egui::Label::new("https://github.com/lorem/pcsx2-mouse-injector").wrap());
                        ui.end_row();

                        ui.label(RichText::new("Description:").strong());
                        ui.add(egui::Label::new("High-performance direct camera look injector for PS2 emulation.").wrap());
                        ui.end_row();
                    });
            });
    }
}
