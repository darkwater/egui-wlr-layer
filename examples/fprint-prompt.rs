use std::{
    io::{BufRead as _, BufReader},
    process::{Command, Stdio},
    time::Duration,
};

use egui::{Color32, FontId, LayerId, Rect, pos2, text::LayoutJob};
use egui_wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerAppOpts, LayerSurface};

struct PositionInfo {
    thickness: u32,
    length: u32,
    edge: Anchor,
    close_to: Anchor,
    offset: u32,
}

const POS: PositionInfo = PositionInfo {
    thickness: 3,
    length: 86,
    // TODO: actually support other orientations
    edge: Anchor::RIGHT,
    close_to: Anchor::TOP,
    offset: 63,
};

const AREA_LENGTH: u32 = 180;

impl PositionInfo {
    const fn win_width(&self) -> u32 {
        match self.edge {
            Anchor::TOP | Anchor::BOTTOM => self.length + self.offset,
            Anchor::LEFT | Anchor::RIGHT => self.thickness + AREA_LENGTH,
            _ => unreachable!(),
        }
    }

    const fn win_height(&self) -> u32 {
        match self.edge {
            Anchor::TOP | Anchor::BOTTOM => self.thickness,
            Anchor::LEFT | Anchor::RIGHT => self.length + self.offset,
            _ => unreachable!(),
        }
    }

    const fn area_width(&self) -> u32 {
        match self.edge {
            Anchor::TOP | Anchor::BOTTOM => self.length,
            Anchor::LEFT | Anchor::RIGHT => AREA_LENGTH,
            _ => unreachable!(),
        }
    }

    const fn area_height(&self) -> u32 {
        match self.edge {
            Anchor::TOP | Anchor::BOTTOM => AREA_LENGTH,
            Anchor::LEFT | Anchor::RIGHT => self.length,
            _ => unreachable!(),
        }
    }
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    // monitor for fprint events
    let monitor_proc = Command::new("dbus-monitor")
        .args(["--system", "interface='net.reactivated.Fprint.Device'"])
        .stdout(Stdio::piped())
        .spawn()?;

    let monitor_stdout = BufReader::new(monitor_proc.stdout.unwrap());

    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        for line in monitor_stdout.lines() {
            let line = line.expect("Failed to read line from dbus-monitor");

            if line.contains("member=VerifyFingerSelected") {
                // fprint is waiting for a finger touch

                tx.send(true);
            }

            if line.contains("member=VerifyStatus") {
                // fprint is no longer waiting for a finger touch

                tx.send(false);
            }
        }

        panic!("dbus monitor thread exited");
    });

    let mut context = egui_wlr_layer::Context::new();

    // let mut layer_app = None;

    context.new_layer_app(
        Box::new(FprintPromptApp),
        LayerAppOpts {
            layer: Layer::Overlay,
            namespace: Some("fprint-prompt"),
            output: Some(&|info: egui_wlr_layer::OutputInfo| {
                info.name == Some("eDP-1".to_string())
            }),
            ..Default::default()
        },
    );

    loop {
        match context.poll_dispatch() {
            Ok(0) => {}
            Ok(n) => {
                println!("handled {} events", n);
            }
            Err(e) => {
                eprintln!("Error polling events: {}", e);
                break Err(e.into());
            }
        }

        if let Ok(waiting) = rx.recv_timeout(Duration::from_millis(1)) {
            if waiting {
                // fprint is waiting for a finger touch
                // layer_app = Some(context.new_layer_app(Box::new(FprintPromptApp)));
            } else {
                // fprint is no longer waiting for a finger touch
                // layer_app.take();
            }
        }
    }
}

struct FprintPromptApp;

impl egui_wlr_layer::App for FprintPromptApp {
    fn update(&mut self, ctx: &egui::Context) {
        ctx.style_mut(|s| {
            s.visuals.panel_fill = Color32::TRANSPARENT;
            s.visuals.widgets.noninteractive.fg_stroke = (1., Color32::WHITE).into();
        });

        let time = ctx.input(|i| i.time) as f32;
        let wiggle = text_animation(time);

        let painter = ctx.layer_painter(LayerId::background());
        let job = painter.layout_job(LayoutJob::simple(
            "Touch to verify".to_string(),
            FontId::proportional(20.),
            Color32::WHITE,
            AREA_LENGTH as f32,
        ));

        painter.galley(
            pos2(
                POS.area_width() as f32 / 2. - wiggle,
                POS.offset as f32 + POS.area_height() as f32 / 2.,
            ) - job.size() / 2.,
            job,
            // Align2::CENTER_CENTER,
            Color32::WHITE,
        );

        let thickness = POS.thickness as f32 * (time * 5.).min(1.0);

        painter.rect_filled(
            Rect::from_two_pos(
                pos2(POS.win_width() as f32, POS.offset as f32),
                pos2(POS.win_width() as f32 - thickness, POS.win_height() as f32),
            ),
            0.,
            Color32::WHITE,
        );

        ctx.request_repaint();
    }

    fn on_init(&mut self, layer: &LayerSurface) {
        layer.set_anchor(POS.edge | POS.close_to);
        layer.set_size(POS.win_width(), POS.win_height());
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    }
}

fn text_animation(time: f32) -> f32 {
    // Configurable parameters
    let speed = 2.0; // Slows down the animation; lower = slower
    let magnitude = 10.; // The maximum distance from the center
    let start_scale = 20.0; // The multiplier at time = 0

    // Scaled time
    let t = time * speed;

    // Phase within one sine wave cycle
    let x = t;
    let sin_x = x.sin();

    // Linearly interpolate scale from `start_scale` to 1.0 over [0, PI/2]
    let scale = if x < std::f32::consts::FRAC_PI_2 {
        start_scale - (start_scale - 1.0) * (x / std::f32::consts::FRAC_PI_2)
    } else {
        1.0
    };

    // Apply transformation and re-center around 0
    ((sin_x - 1.0) * scale + 1.0) * magnitude
}
