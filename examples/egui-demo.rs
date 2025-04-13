use egui_wlr_layer::{Anchor, Layer, LayerAppOpts};

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut context = egui_wlr_layer::Context::new();

    context.new_layer_app(Box::new(DemoApp(Default::default())), LayerAppOpts {
        layer: Layer::Top,
        namespace: Some("egui-demo"),
        output: None,
    });

    loop {
        match context.blocking_dispatch() {
            Ok(0) => {}
            Ok(n) => {
                println!("handled {} events", n);
            }
            Err(e) => {
                eprintln!("Error polling events: {}", e);
                break Err(e.into());
            }
        }
    }
}

struct DemoApp(egui_demo_lib::DemoWindows);

impl egui_wlr_layer::App for DemoApp {
    fn update(&mut self, ctx: &egui::Context) {
        self.0.ui(ctx);
    }

    fn on_init(&mut self, layer: &smithay_client_toolkit::shell::wlr_layer::LayerSurface) {
        layer.set_anchor(Anchor::all());
    }
}
