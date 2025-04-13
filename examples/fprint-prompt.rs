use std::{
    io::{BufRead as _, BufReader},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

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

    context.new_layer_app(Box::new(FprintPromptApp {
        demo: Default::default(),
        last_frame: Instant::now(),
    }));

    loop {
        match context.poll_events() {
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

struct FprintPromptApp {
    demo: egui_demo_lib::DemoWindows,
    last_frame: Instant,
}

impl egui_wlr_layer::App for FprintPromptApp {
    fn update(&mut self, ctx: &egui::Context) {
        self.demo.ui(ctx);

        let painter = ctx.debug_painter();
        painter.line_segment(
            [ctx.screen_rect().left_top(), ctx.screen_rect().right_bottom()],
            (1.0, egui::Color32::WHITE),
        );
        painter.line_segment(
            [ctx.screen_rect().left_bottom(), ctx.screen_rect().right_top()],
            (1.0, egui::Color32::WHITE),
        );
        painter.text(
            egui::pos2(0., 100.),
            egui::Align2::LEFT_TOP,
            painter.clip_rect(),
            egui::FontId::proportional(12.),
            egui::Color32::WHITE,
        );
        painter.text(
            egui::pos2(0., 120.),
            egui::Align2::LEFT_TOP,
            format!("Frame time: {:?}", self.last_frame.elapsed()),
            egui::FontId::proportional(12.),
            egui::Color32::WHITE,
        );

        // egui::CentralPanel::default().show(ctx, |ui| {
        //     ui.label("Please touch the fingerprint reader");
        //     ui.horizontal(|ui| {
        //         if ui.button("Cancel").clicked() {
        //             std::process::exit(0);
        //         }
        //     });

        //     ui.collapsing("Spinner", |ui| {
        //         ui.spinner();
        //     });

        //     ui.label(ctx.pixels_per_point().to_string());

        //     ui.label(ctx.debug_painter().clip_rect().to_string());

        //     egui::ScrollArea::new([false, true]).show(ui, |ui| {
        //         ctx.settings_ui(ui);
        //     });

        //     // let painter = ui.painter();
        //     // painter.line_segment(
        //     //     [ui.clip_rect().left_top(), ui.clip_rect().right_bottom()],
        //     //     (1.0, egui::Color32::WHITE),
        //     // );
        //     // painter.line_segment(
        //     //     [ui.clip_rect().left_bottom(), ui.clip_rect().right_top()],
        //     //     (1.0, egui::Color32::WHITE),
        //     // );
        //     // painter.line_segment(
        //     //     [egui::pos2(20., 20.), egui::pos2(200., 20.)],
        //     //     (1.0, egui::Color32::WHITE),
        //     // );
        //     // painter.line_segment(
        //     //     [egui::pos2(20., 30.), egui::pos2(200., 30.)],
        //     //     (2.0, egui::Color32::WHITE),
        //     // );
        //     // painter.line_segment(
        //     //     [
        //     //         egui::pos2(20., 60. / ctx.pixels_per_point()),
        //     //         egui::pos2(200., 60. / ctx.pixels_per_point()),
        //     //     ],
        //     //     (1. / ctx.pixels_per_point(), egui::Color32::WHITE),
        //     // );
        //     // painter.line_segment(
        //     //     [
        //     //         egui::pos2(20., 80. / ctx.pixels_per_point()),
        //     //         egui::pos2(200., 80. / ctx.pixels_per_point()),
        //     //     ],
        //     //     (2. / ctx.pixels_per_point(), egui::Color32::WHITE),
        //     // );
        // });

        self.last_frame = Instant::now();
    }
}
