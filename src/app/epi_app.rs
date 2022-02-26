use eframe::egui::style::Margin;
use eframe::epaint::{Color32, Stroke};
use eframe::{
    egui::{self, Frame},
    epi,
};

use super::*;

impl epi::App for VSPreviewer {
    fn name(&self) -> &str {
        "vspreview-rs"
    }

    fn setup(
        &mut self,
        ctx: &egui::Context,
        frame: &epi::Frame,
        _storage: Option<&dyn epi::Storage>,
    ) {
        // Load existing or default state
        if let Some(storage) = _storage {
            self.state = epi::get_value(storage, epi::APP_KEY).unwrap_or(PreviewState {
                zoom_factor: 1.0,
                zoom_multiplier: 1.0,
                scroll_multiplier: 1.0,
                canvas_margin: 0.0,
                ..Default::default()
            });
        }

        // Set the global theme, default to dark mode
        let mut global_visuals = egui::style::Visuals::dark();
        global_visuals.window_shadow = egui::epaint::Shadow::small_light();
        ctx.set_visuals(global_visuals);

        // Fix invalid state options
        if self.state.scroll_multiplier <= 0.0 {
            self.state.scroll_multiplier = 1.0;
        }

        // Limit to 2.0 multiplier every zoom, should be plenty
        if self.state.zoom_multiplier < 1.0 {
            self.state.zoom_multiplier = 1.0;
        } else if self.state.zoom_multiplier > 2.0 {
            self.state.zoom_multiplier = 2.0;
        }

        // Request initial outputs
        self.reload(frame.clone());
    }

    fn update(&mut self, ctx: &egui::Context, frame: &epi::Frame) {
        let cur_output = self.state.cur_output;

        // Initial callback
        self.check_reload_finish();

        // We want a new frame
        // Previously rendering frames must have completed to request a new one
        self.try_rerender(frame);

        // Poll new requested frame, replace old if ready
        self.check_rerender_finish(ctx);

        // Check for original props if requested
        self.check_original_props_finish();

        let has_current_output = !self.outputs.is_empty() && self.outputs.contains_key(&cur_output);
        let panel_frame = Frame::default()
            .fill(Color32::from_gray(51))
            .margin(Margin::same(self.state.canvas_margin))
            .stroke(Stroke::none());

        egui::CentralPanel::default()
            .frame(panel_frame)
            .show(ctx, |ui| {
                // Check for quit, GUI toggle, etc.
                self.check_misc_keyboard_inputs(frame, ui);

                // React on canvas resolution change
                if self.available_size != ui.available_size() {
                    self.available_size = ui.available_size();

                    self.reprocess_outputs();
                }

                // Draw window on top
                if self.state.show_gui {
                    UiStateWindow::ui(self, ctx, frame);
                }

                // Centered image painted on
                UiPreviewImage::ui(self, frame, ui);

                // Bottom panel
                if self.state.show_gui && has_current_output {
                    UiBottomPanel::ui(self, ctx);
                }

                // Check at the end of frame for reprocessing
                self.try_rerender(frame);
            });
    }

    fn save(&mut self, storage: &mut dyn epi::Storage) {
        epi::set_value(storage, epi::APP_KEY, &self.state);
    }
}
