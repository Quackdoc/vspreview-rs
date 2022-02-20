use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use eframe::egui::style::Margin;
use eframe::egui::Key;
use eframe::egui::Ui;
use eframe::egui::Visuals;
use eframe::epaint::Color32;
use eframe::epaint::ColorImage;
use eframe::epaint::Stroke;
use eframe::epaint::Vec2;
use eframe::{
    egui::{self, Frame},
    epi,
};
use poll_promise::Promise;

use crate::utils::{process_image, MAX_ZOOM, MIN_ZOOM};
use crate::vs_handler::PreviewedScript;
use crate::vs_handler::VSOutput;

type APreviewFrame = Arc<PreviewFrame>;
type FramePromise = Promise<APreviewFrame>;

#[derive(Default)]
pub struct Previewer {
    pub script: Arc<Mutex<PreviewedScript>>,
    pub reload_data: Option<Promise<(HashMap<i32, VSOutput>, APreviewFrame)>>,
    pub state: PreviewState,

    pub initialized: bool,

    pub outputs: HashMap<i32, PreviewOutput>,
    pub last_output_key: i32,

    pub rerender: bool,
    pub reprocess: bool,
    pub replace_frame_promise: Option<FramePromise>,

    pub available_size: Vec2,
}

#[derive(Default, Clone, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct PreviewState {
    pub cur_output: i32,
    pub cur_frame_no: u32,

    pub scale_to_window: bool,

    /// Defaults to Point
    pub scale_filter: PreviewFilterType,

    pub zoom_factor: f32,

    pub translate: Vec2,
    pub scroll_multiplier: f32,
    pub canvas_margin: f32,
}

#[derive(Default)]
pub struct PreviewOutput {
    pub vsoutput: VSOutput,

    pub frame_promise: Option<FramePromise>,

    pub force_reprocess: bool,
    pub last_frame_no: u32,
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub enum PreviewFilterType {
    Point,
    Bilinear,
    Hamming,
    CatmullRom,
    Mitchell,
    Lanczos3,
}

#[derive(Clone)]
pub struct PreviewFrame {
    // Thread safe as always immutable
    pub image: Arc<ColorImage>,

    pub texture: egui::TextureHandle,
    pub frame_type: String,
}

impl epi::App for Previewer {
    fn name(&self) -> &str {
        "vspreview-rs"
    }

    fn setup(
        &mut self,
        ctx: &egui::Context,
        frame: &epi::Frame,
        _storage: Option<&dyn epi::Storage>,
    ) {
        if let Some(storage) = _storage {
            self.state = epi::get_value(storage, epi::APP_KEY).unwrap_or(PreviewState {
                scale_to_window: true,
                zoom_factor: 1.0,
                scroll_multiplier: 2.0,
                canvas_margin: 2.0,
                ..Default::default()
            })
        }

        self.state.cur_frame_no = 12345;
        self.state.zoom_factor = 1.0;
        self.state.scale_to_window = false;
        self.state.translate = Vec2::ZERO;
        self.state.canvas_margin = 10.0;

        if self.state.scroll_multiplier <= 0.0 {
            self.state.scroll_multiplier = 1.0;
        }

        self.reload(ctx.clone(), frame.clone(), true);
    }

    fn update(&mut self, ctx: &egui::Context, frame: &epi::Frame) {
        let cur_output = self.state.cur_output;

        // Initial callback
        self.check_reload_finish();

        // We want a new frame
        // Previously rendering frames must have completed
        self.check_rerender(ctx, frame);

        // Poll new requested frame, replace old if ready
        if let Some(promise) = self.replace_frame_promise.as_ref() {
            if promise.poll().is_ready() {
                let output = self.outputs.get_mut(&cur_output).unwrap();
                output.frame_promise = Some(self.replace_frame_promise.take().unwrap());

                // Update last output once the new frame is rendered
                self.last_output_key = cur_output;
            }
        }

        let has_current_output = !self.outputs.is_empty() && self.outputs.contains_key(&cur_output);

        // If the outputs differ in frame index, we should wait for the render
        // instead of rendering the old frame
        let output_diff_frame = if has_current_output {
            let cur_output = self.outputs.get(&cur_output).unwrap();
            let last_output = self.outputs.get(&self.last_output_key).unwrap();

            last_output.last_frame_no != cur_output.last_frame_no
        } else {
            false
        };

        let new_frame = Frame::default()
            .fill(Color32::from_gray(150))
            .margin(Margin::symmetric(0.0, 0.0))
            .stroke(Stroke::none());

        egui::CentralPanel::default()
            .frame(new_frame)
            .show(ctx, |ui| {
                if ui.input().key_pressed(Key::Q) || ui.input().key_pressed(Key::Escape) {
                    frame.quit();
                }

                let zoom_delta = ui.input().zoom_delta();
                let scroll_delta = ui.input().scroll_delta;

                *ui.visuals_mut() = Visuals::dark();

                // React on canvas resolution change
                if self.available_size != ui.available_size() {
                    self.available_size = ui.available_size();
                    self.outputs
                        .values_mut()
                        .for_each(|o| o.force_reprocess = true);
                }

                ui.centered_and_justified(|ui| {
                    // Acquire frame texture to render now
                    let frame_promise = if has_current_output {
                        let output = self.outputs.get_mut(&cur_output).unwrap();

                        if output_diff_frame {
                            None
                        } else {
                            output.frame_promise.as_ref()
                        }
                    } else {
                        None
                    };

                    if self.reload_data.is_some() || frame_promise.is_none() {
                        ui.add(egui::Spinner::new().size(200.0));
                    } else if let Some(promise) = frame_promise {
                        if let Some(pf) = promise.ready() {
                            let image_size: [f32; 2] = pf.image.size.map(|i| i as f32);

                            let tex_size = pf.texture.size_vec2();
                            ui.image(&pf.texture, tex_size);

                            if !self.rerender && self.replace_frame_promise.is_none() {
                                self.handle_keypresses(ui);
                                self.handle_mouse_inputs(
                                    ui,
                                    Vec2::from(image_size),
                                    zoom_delta,
                                    scroll_delta,
                                );

                                if ui.input().key_pressed(Key::R) {
                                    self.reload(ctx.clone(), frame.clone(), true)
                                }

                                // Check at the end of frame for reprocessing
                                self.check_rerender(ctx, frame);
                            }
                        }
                    }
                });
            });
    }

    fn save(&mut self, storage: &mut dyn epi::Storage) {
        epi::set_value(storage, epi::APP_KEY, &self.state);
    }
}

impl Previewer {
    fn reload(&mut self, ctx: egui::Context, frame: epi::Frame, force_reload: bool) {
        let state = self.state.clone();
        let cur_output = state.cur_output;
        let cur_frame_no = state.cur_frame_no;

        let script = self.script.clone();
        let win_size = self.available_size;

        self.reload_data = Some(poll_promise::Promise::spawn_thread(
            "initialization/reload",
            move || {
                // This is OK because we didn't have an initial texture
                let mut mutex = script.lock().unwrap();

                if force_reload || !mutex.is_initialized() {
                    mutex.reload();
                }

                let outputs = mutex.get_outputs();
                assert!(!outputs.is_empty());

                let output = if !outputs.contains_key(&cur_output) {
                    // Fallback to first output in order
                    let mut keys: Vec<&i32> = outputs.keys().collect();
                    keys.sort();

                    **keys.first().unwrap()
                } else {
                    cur_output
                };

                let vsframe = mutex.get_frame(output, cur_frame_no).unwrap();
                let image = Arc::new(vsframe.frame_image.clone());

                // Return unprocess while we don't have a proper window size
                let processed_image = if win_size.min_elem() > 0.0 {
                    process_image(image.clone(), state, win_size)
                } else {
                    vsframe.frame_image
                };

                let pf = PreviewFrame {
                    image,
                    texture: ctx.load_texture("initial_frame", processed_image),
                    frame_type: vsframe.frame_type,
                };

                frame.request_repaint();

                (outputs, Arc::new(pf))
            },
        ));
    }

    fn check_reload_finish(&mut self) {
        if let Some(promise) = &self.reload_data {
            if let Some(data) = promise.ready() {
                self.outputs = data
                    .0
                    .iter()
                    .map(|(key, o)| {
                        let new = PreviewOutput {
                            vsoutput: o.clone(),
                            ..Default::default()
                        };

                        (*key, new)
                    })
                    .collect();

                println!("Got outputs: {:?}", &self.outputs.len());
                self.outputs
                    .values()
                    .for_each(|o| println!("{:?}", o.vsoutput));

                if !data.0.contains_key(&self.state.cur_output) {
                    // Fallback to first output in order
                    let mut keys: Vec<&i32> = data.0.keys().collect();
                    keys.sort();

                    self.state.cur_output = **keys.first().unwrap();
                }

                let output = self.outputs.get_mut(&self.state.cur_output).unwrap();
                let node_info = &output.vsoutput.node_info;

                let (sender, promise) = Promise::new();
                sender.send(data.1.clone());

                output.frame_promise = Some(promise);

                self.reload_data = None;
                self.last_output_key = self.state.cur_output;

                if self.state.cur_frame_no >= node_info.num_frames {
                    self.state.cur_frame_no = node_info.num_frames - 1;
                }

                // First reload
                if !self.initialized {
                    self.initialized = true;

                    // Force rerender once we have the initial window size
                    if self.state.scale_to_window {
                        self.rerender = true;
                    }
                }
            }
        }
    }

    fn check_rerender(&mut self, ctx: &egui::Context, frame: &epi::Frame) {
        if !self.outputs.is_empty() {
            let output = self.outputs.get_mut(&self.state.cur_output).unwrap();

            if output.force_reprocess {
                self.rerender = true;
                self.reprocess = true;
                output.force_reprocess = false;
            }
        }

        if self.rerender && self.replace_frame_promise.is_none() {
            self.rerender = false;

            let reprocess = self.reprocess;
            self.reprocess = false;

            let script = self.script.clone();
            let win_size = self.available_size;

            let pf = if reprocess {
                let frame = self.get_current_frame();

                if let Some(pf) = &frame {
                    // Ignore translate when image fits already
                    if win_size.x >= pf.image.size[0] as f32 {
                        self.state.translate.x = 0.0;
                    }
                    if win_size.y >= pf.image.size[1] as f32 {
                        self.state.translate.y = 0.0;
                    }
                }

                frame
            } else {
                None
            };

            let state = self.state.clone();

            let ctx = ctx.clone();
            let frame = frame.clone();

            self.replace_frame_promise = Some(poll_promise::Promise::spawn_thread(
                "fetch_frame",
                move || Self::get_preview_image(ctx, frame, script, state, pf, reprocess, win_size),
            ));
        }
    }

    fn get_current_frame(&self) -> Option<APreviewFrame> {
        if !self.outputs.is_empty() {
            let output = self.outputs.get(&self.state.cur_output).unwrap();

            // Already have a frame
            if let Some(p) = &output.frame_promise {
                p.ready().cloned()
            } else {
                None
            }
        } else {
            None
        }
    }

    fn get_preview_image(
        ctx: egui::Context,
        frame: epi::Frame,
        script: Arc<Mutex<PreviewedScript>>,
        state: PreviewState,
        pf: Option<APreviewFrame>,
        reprocess: bool,
        win_size: Vec2,
    ) -> APreviewFrame {
        // This is fine because only one promise may be executing at a time
        let mut mutex = script.lock().unwrap();

        let have_existing_frame = pf.is_some();

        // Reuse existing image, process and recreate texture
        let pf = if reprocess && have_existing_frame {
            let pf = pf.unwrap();
            let processed_image = process_image(pf.image.clone(), state, win_size);

            PreviewFrame {
                image: pf.image.clone(),
                texture: ctx.load_texture("frame", processed_image),
                frame_type: pf.frame_type.clone(),
            }
        } else {
            // Request new frame, process and recreate texture
            let vsframe = mutex
                .get_frame(state.cur_output, state.cur_frame_no)
                .unwrap();
            let image = Arc::new(vsframe.frame_image);
            let processed_image = process_image(image.clone(), state, win_size);

            PreviewFrame {
                image,
                texture: ctx.load_texture("frame", processed_image),
                frame_type: vsframe.frame_type,
            }
        };

        // Once frame is ready
        frame.request_repaint();

        Arc::new(pf)
    }

    fn handle_keypresses(&mut self, ui: &mut Ui) {
        let mut rerender = self.check_update_seek(ui);
        rerender |= self.check_update_output(ui);

        self.rerender = rerender;
    }

    /// Returns whether to rerender
    fn check_update_seek(&mut self, ui: &mut Ui) -> bool {
        // Must not have modifiers
        if !ui.input().modifiers.is_none() {
            return false;
        }

        let output = self.outputs.get_mut(&self.state.cur_output).unwrap();
        let node_info = &output.vsoutput.node_info;

        let current = self.state.cur_frame_no;

        let res = if ui.input().key_pressed(Key::ArrowLeft) || ui.input().key_pressed(Key::H) {
            if current > 0 {
                self.state.cur_frame_no -= 1;
                true
            } else {
                false
            }
        } else if ui.input().key_pressed(Key::ArrowRight) || ui.input().key_pressed(Key::L) {
            if current < node_info.num_frames - 1 {
                self.state.cur_frame_no += 1;
                true
            } else {
                false
            }
        } else if ui.input().key_pressed(Key::ArrowUp) | ui.input().key_pressed(Key::K) {
            if current >= node_info.framerate {
                self.state.cur_frame_no -= node_info.framerate;
                true
            } else if current < node_info.framerate {
                self.state.cur_frame_no = 0;
                true
            } else {
                false
            }
        } else if ui.input().key_pressed(Key::ArrowDown) | ui.input().key_pressed(Key::J) {
            self.state.cur_frame_no += node_info.framerate;

            self.state.cur_frame_no < node_info.num_frames - 1
        } else {
            false
        };

        // Update frame once it's loaded
        output.last_frame_no = current;

        self.state.cur_frame_no = self.state.cur_frame_no.clamp(0, node_info.num_frames - 1);

        res
    }

    fn check_update_output(&mut self, ui: &mut Ui) -> bool {
        // Must not have modifiers
        if !ui.input().modifiers.is_none() {
            return false;
        }

        let old_output = self.state.cur_output;

        let new_output: i32 = if ui.input().key_pressed(Key::Num1) {
            0
        } else if ui.input().key_pressed(Key::Num2) {
            1
        } else if ui.input().key_pressed(Key::Num3) {
            2
        } else if ui.input().key_pressed(Key::Num4) {
            3
        } else if ui.input().key_pressed(Key::Num5) {
            4
        } else if ui.input().key_pressed(Key::Num6) {
            5
        } else if ui.input().key_pressed(Key::Num7) {
            6
        } else if ui.input().key_pressed(Key::Num8) {
            7
        } else if ui.input().key_pressed(Key::Num9) {
            8
        } else if ui.input().key_pressed(Key::Num0) {
            9
        } else {
            -1
        };

        let mut res = if new_output >= 0 && self.outputs.contains_key(&new_output) {
            self.state.cur_output = new_output;

            true
        } else {
            false
        };

        // Changed output
        if res {
            let old = self.outputs.get(&old_output).unwrap();
            let new = self.outputs.get(&self.state.cur_output).unwrap();

            res = old.last_frame_no != new.last_frame_no;
        }

        res
    }

    /// Size of the image to scroll/zoom, not the final texture
    fn handle_mouse_inputs(
        &mut self,
        ui: &mut Ui,
        size: Vec2,
        zoom_delta: f32,
        scroll_delta: Vec2,
    ) {
        let res = if zoom_delta != 1.0 {
            // Zoom
            let mut delta = zoom_delta;
            let mut new_factor = self.state.zoom_factor;

            let zoom_modifier = if ui.input().key_pressed(Key::ArrowDown) {
                delta = 0.0;
                0.1
            } else if ui.input().key_pressed(Key::ArrowUp) {
                delta = 2.0;
                0.1
            } else {
                1.0
            };

            // Ignore 1.0 delta, means no zoom done
            if delta < 1.0 {
                // Smaller unzooming when below 1.0
                if new_factor <= 1.0 {
                    new_factor -= 0.125;
                } else {
                    new_factor -= zoom_modifier;
                }

                new_factor = new_factor.clamp(MIN_ZOOM, MAX_ZOOM);
            } else if delta > 1.0 {
                if new_factor < 1.0 {
                    // Zoom back from a unzoomed state
                    // Go back to no zoom
                    new_factor += 0.125;
                } else {
                    new_factor += zoom_modifier;
                }

                new_factor = new_factor.clamp(MIN_ZOOM, MAX_ZOOM);
            }

            if new_factor != self.state.zoom_factor {
                let trunc_factor = if new_factor < 1.0 { 1000.0 } else { 10.0 };
                self.state.zoom_factor = (new_factor * trunc_factor).round() / trunc_factor;

                !(self.state.scale_to_window && self.state.zoom_factor < 1.0)
            } else {
                false
            }
        } else if scroll_delta.length() > 0.0 {
            self.state.translate -= scroll_delta * self.state.scroll_multiplier;
            let margin = self.state.canvas_margin.abs();

            // Left and right clipped
            let max_tx = (size.x - self.available_size.x).abs();

            // Clips at the bottom only vertically
            let max_ty = (size.y - self.available_size.y).abs();

            // With 2px margin to be able to see the edge
            self.state.translate.x = if max_tx.is_sign_positive() {
                self.state
                    .translate
                    .x
                    .clamp(-max_tx - margin, max_tx + margin)
            } else {
                // Negative means the image isn't clipped by the window rect
                self.state.translate.x.clamp(0.0, 0.0)
            };

            self.state.translate.y = if max_ty.is_sign_positive() {
                self.state
                    .translate
                    .y
                    .clamp(-max_ty - margin, max_ty + margin)
            } else {
                // Negative means the image isn't clipped by the window rect
                self.state.translate.y.clamp(0.0, 0.0)
            };

            true
        } else {
            false
        };

        // Set other outputs to reprocess if we're modifying the image
        if res {
            self.outputs
                .values_mut()
                .for_each(|out| out.force_reprocess = true);
        }

        self.rerender |= res;
    }
}

impl Default for PreviewFilterType {
    fn default() -> Self {
        PreviewFilterType::Point
    }
}

impl From<PreviewFilterType> for fast_image_resize::FilterType {
    fn from(f: PreviewFilterType) -> Self {
        match f {
            PreviewFilterType::Point => fast_image_resize::FilterType::Box,
            PreviewFilterType::Bilinear => fast_image_resize::FilterType::Bilinear,
            PreviewFilterType::Hamming => fast_image_resize::FilterType::Hamming,
            PreviewFilterType::CatmullRom => fast_image_resize::FilterType::CatmullRom,
            PreviewFilterType::Mitchell => fast_image_resize::FilterType::Mitchell,
            PreviewFilterType::Lanczos3 => fast_image_resize::FilterType::Lanczos3,
        }
    }
}
