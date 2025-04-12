use std::{
    io::{BufRead as _, BufReader},
    process::{Command, Stdio},
    time::Duration,
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

    let mut layer_app = None;

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

        if let Ok(waiting) = rx.recv_timeout(Duration::from_millis(10)) {
            if waiting {
                // fprint is waiting for a finger touch
                layer_app = Some(context.new_layer_app(Box::new(FprintPromptApp)));
            } else {
                // fprint is no longer waiting for a finger touch
                layer_app.take();
            }
        }
    }
}

struct FprintPromptApp;

impl egui_wlr_layer::App for FprintPromptApp {
    fn update(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("Please touch the fingerprint reader");
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    std::process::exit(0);
                }
            });
        });
    }
}
