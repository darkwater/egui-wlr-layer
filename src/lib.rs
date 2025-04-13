use std::{collections::HashMap, io::ErrorKind, ptr::NonNull, time::Instant};

use egui::AreaState;
use egui_wgpu::{ScreenDescriptor, WgpuConfiguration, wgpu::TextureFormat};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat,
    output::{OutputHandler, OutputState},
    reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1,
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{LayerShell, LayerShellHandler, LayerSurfaceConfigure},
    },
};
pub use smithay_client_toolkit::{
    output::OutputInfo,
    shell::wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerSurface},
};
use wayland_backend::client::{ObjectId, WaylandError};
use wayland_client::{
    Connection, DispatchError, EventQueue, Proxy as _, QueueHandle,
    globals::registry_queue_init,
    protocol::{
        wl_keyboard,
        wl_output::{self},
        wl_pointer, wl_seat, wl_surface,
    },
};
use wgpu::{
    CompositeAlphaMode,
    rwh::{RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle},
};

use self::{wp_fractional_scaling::FractionalScalingManager, wp_viewporter::ViewporterState};

mod wp_fractional_scaling;
mod wp_viewporter;

const DEFAULT_WIDTH: u32 = 1920;
const DEFAULT_HEIGHT: u32 = 1080;

pub struct Context {
    event_queue: EventQueue<ContextDelegate>,
    delegate: ContextDelegate,
}

struct ContextDelegate {
    wayland_conn: Connection,
    compositor: CompositorState,
    layer_shell: LayerShell,
    fractional_scaling: FractionalScalingManager,
    viewporter: ViewporterState,
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    wgpu_instance: wgpu::Instance,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    apps: HashMap<ObjectId, LayerApp>,
}

impl ContextDelegate {
    fn scale_factor_changed(
        &mut self,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: f32,
    ) {
        if let Some(app) = self.apps.get_mut(&surface.id()) {
            if app.scale == new_factor {
                // No change
                return;
            }

            println!("Scale factor changed to {new_factor}");

            let viewport = self.viewporter.get_viewport(app.layer.wl_surface(), qh);
            viewport.set_destination(app.width as i32, app.height as i32);

            app.scale = new_factor;
            app.draw(&self.compositor);
        }
    }
}

/// Whether/how to use input regions, can be used to let mouse and touch inputs fall through the
/// layer surface.
#[derive(Debug, Clone, Copy, Default)]
pub enum InputRegions {
    /// The entire surface takes mouse inputs.
    #[default]
    Full,
    /// The background layer doesn't take inputs, only windows and popups do. This means that eg.
    /// CentralPanels and SidePanels will not take inputs, because they're typically drawn on the
    /// background layer.
    WindowsOnly,
    /// The layer surface doesn't take any mouse or touch inputs at all.
    None,
    // TODO: add more options (select layers, custom behaviour)
}

pub struct LayerAppOpts<'a> {
    pub layer: Layer,
    pub namespace: Option<&'a str>,
    pub output: Option<&'a dyn Fn(OutputInfo) -> bool>,
    pub input_regions: InputRegions,
}

impl Default for LayerAppOpts<'_> {
    fn default() -> Self {
        Self {
            layer: Layer::Top,
            namespace: Default::default(),
            output: Default::default(),
            input_regions: InputRegions::Full,
        }
    }
}

// pub type OutputSelector = Box<dyn Fn(OutputInfo) -> bool>;

impl Context {
    pub fn new() -> Self {
        // All Wayland apps start by connecting the compositor (server).
        // TODO: reuse between instancces?
        let wayland_conn = Connection::connect_to_env().unwrap();

        // Enumerate the list of globals to get the protocols the server implements.
        let (globals, event_queue) = registry_queue_init(&wayland_conn).unwrap();

        let wgpu_instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let qh = event_queue.handle();

        let compositor =
            CompositorState::bind(&globals, &qh).expect("wl_compositor is not available");

        let layer_shell = LayerShell::bind(&globals, &qh).expect("layer shell is not available");

        let fractional_scaling = FractionalScalingManager::bind(&globals, &qh).unwrap();
        let viewporter = ViewporterState::bind(&globals, &qh).unwrap();

        Context {
            event_queue,
            delegate: ContextDelegate {
                wayland_conn,
                compositor,
                layer_shell,
                fractional_scaling,
                viewporter,
                registry_state: RegistryState::new(&globals),
                seat_state: SeatState::new(&globals, &qh),
                output_state: OutputState::new(&globals, &qh),
                wgpu_instance,
                keyboard: None,
                pointer: None,
                apps: HashMap::new(),
            },
        }
    }

    pub fn new_layer_app(
        &mut self,
        mut app: Box<dyn App>,
        LayerAppOpts {
            layer,
            namespace,
            output,
            input_regions,
        }: LayerAppOpts,
    ) {
        let qh = self.event_queue.handle();

        // A layer surface is created from a surface.
        let wl_surface = self.delegate.compositor.create_surface(&qh);

        let output = output.and_then(|selector| {
            self.delegate
                .output_state
                .outputs()
                .filter_map(|output| {
                    self.delegate
                        .output_state
                        .info(&output)
                        .map(|info| (info, output))
                })
                .find_map(|(info, output)| selector(info).then_some(output))
        });

        // And then we create the layer shell.
        let layer = self.delegate.layer_shell.create_layer_surface(
            &qh,
            wl_surface,
            layer,
            namespace,
            output.as_ref(),
        );

        app.on_init(&layer);

        let raw_display_handle = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(self.delegate.wayland_conn.backend().display_ptr() as *mut _).unwrap(),
        ));
        let raw_window_handle = RawWindowHandle::Wayland(WaylandWindowHandle::new(
            NonNull::new(layer.wl_surface().id().as_ptr() as *mut _).unwrap(),
        ));

        let wgpu_surface = unsafe {
            self.delegate
                .wgpu_instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle,
                    raw_window_handle,
                })
                .expect("Failed to create wgpu surface")
        };

        // // TODO: make this function async instead of block on these?
        let egui_context = egui::Context::default();

        {
            let surface = layer.wl_surface().clone();
            let qh = qh.clone();
            egui_context.set_request_repaint_callback(move |_info| {
                // TODO: handle info.delay
                surface.frame(&qh, surface.clone());
            });
        }

        let msaa_samples = 1;
        let dithering = true;
        let egui_render_state = pollster::block_on(egui_wgpu::RenderState::create(
            &WgpuConfiguration::default(),
            &self.delegate.wgpu_instance,
            Some(&wgpu_surface),
            None,
            msaa_samples,
            dithering,
        ))
        .expect("Failed to create egui render state");

        // In order for the layer surface to be mapped, we need to perform an initial commit with no attached\
        // buffer. For more info, see WaylandSurface::commit
        //
        // The compositor will respond with an initial configure that we can then use to present to the layer
        // surface with the correct options.
        layer.commit();

        let fractional_scale = self
            .delegate
            .fractional_scaling
            .fractional_scaling(layer.wl_surface(), &qh);

        self.delegate.apps.insert(
            layer.wl_surface().id(),
            LayerApp {
                app,
                wgpu_surface,
                // wgpu_adapter,
                // wgpu_device,
                // wgpu_queue,
                egui_context,
                egui_render_state,
                layer,
                fractional_scale,

                start: Instant::now(),
                events: Vec::new(),
                modifiers: egui::Modifiers::default(),
                input_regions,
                exit: false,
                first_configure: true,
                width: DEFAULT_WIDTH,
                height: DEFAULT_HEIGHT,
                scale: 1.,
                shift: None,
                keyboard_focus: false,
            },
        );
    }

    pub fn poll_dispatch(&mut self) -> Result<usize, DispatchError> {
        let dispatched = self.event_queue.dispatch_pending(&mut self.delegate)?;
        if dispatched > 0 {
            return Ok(dispatched);
        }

        self.delegate.wayland_conn.flush()?;

        if let Some(guard) = self.delegate.wayland_conn.prepare_read() {
            match guard.read() {
                Ok(_) => self.event_queue.dispatch_pending(&mut self.delegate),
                Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => Ok(0),
                Err(e) => Err(e.into()),
            }
        } else {
            Ok(0)
        }
    }

    pub fn blocking_dispatch(&mut self) -> Result<usize, DispatchError> {
        self.event_queue.blocking_dispatch(&mut self.delegate)
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(unused_variables)]
pub trait App {
    fn update(&mut self, ctx: &egui::Context);

    fn on_init(&mut self, layer: &LayerSurface) {}
}

pub struct LayerApp {
    app: Box<dyn App>,
    wgpu_surface: wgpu::Surface<'static>,
    // wgpu_adapter: wgpu::Adapter,
    // wgpu_device: wgpu::Device,
    // wgpu_queue: wgpu::Queue,
    egui_context: egui::Context,
    egui_render_state: egui_wgpu::RenderState,
    layer: LayerSurface, // drop after wgpu_surface
    #[allow(dead_code)] // just needs to stay alive
    fractional_scale: WpFractionalScaleV1,

    start: Instant,
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
    input_regions: InputRegions,
    exit: bool,
    first_configure: bool,
    width: u32,
    height: u32,
    scale: f32,
    shift: Option<u32>,
    keyboard_focus: bool,
}

impl LayerApp {
    fn physical_width(&self) -> u32 {
        (self.width as f32 * self.scale) as u32
    }

    fn physical_height(&self) -> u32 {
        (self.height as f32 * self.scale) as u32
    }

    fn draw(&mut self, compositor: &CompositorState) {
        // TODO: input
        let raw_input = egui::RawInput {
            time: Some(self.start.elapsed().as_secs_f64()),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0., 0.),
                egui::vec2(self.width as f32, self.height as f32),
            )),
            events: std::mem::take(&mut self.events),
            ..Default::default()
        };

        self.egui_context.set_pixels_per_point(self.scale);

        // let adapter = &self.egui_render_state.adapter;
        let surface = &self.wgpu_surface;
        let device = &self.egui_render_state.device;
        let queue = &self.egui_render_state.queue;

        let full_output = self.egui_context.run(raw_input, |ctx| self.app.update(ctx));

        // TODO: handle full_output.platform_output

        let paint_jobs = self
            .egui_context
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: TextureFormat::Bgra8Unorm,
            view_formats: vec![TextureFormat::Bgra8Unorm],
            alpha_mode: CompositeAlphaMode::PreMultiplied,
            width: self.physical_width(),
            height: self.physical_height(),
            desired_maximum_frame_latency: 2,
            // Wayland is inherently a mailbox system.
            present_mode: wgpu::PresentMode::Mailbox,
        };

        surface.configure(device, &surface_config);

        let surface_texture = surface
            .get_current_texture()
            .expect("failed to acquire next swapchain texture");

        let texture_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor {
                format: Some(surface_config.format),
                ..Default::default()
            });

        let mut encoder = device.create_command_encoder(&Default::default());

        for (id, image_delta) in &full_output.textures_delta.set {
            self.egui_render_state
                .renderer
                .write()
                .update_texture(device, queue, *id, image_delta);
        }

        let screen_descriptor = ScreenDescriptor {
            size_in_pixels: [self.physical_width(), self.physical_height()],
            pixels_per_point: self.scale,
        };

        self.egui_render_state.renderer.write().update_buffers(
            device,
            queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        {
            let mut render_pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &texture_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .forget_lifetime();

            self.egui_render_state.renderer.read().render(
                &mut render_pass,
                &paint_jobs,
                &screen_descriptor,
            );
        }

        for x in &full_output.textures_delta.free {
            self.egui_render_state.renderer.write().free_texture(x)
        }

        match self.input_regions {
            InputRegions::Full => self.layer.set_input_region(None),
            InputRegions::WindowsOnly => {
                if let Ok(region) = Region::new(compositor) {
                    let layers = self
                        .egui_context
                        .memory(|memory| {
                            let areas = memory.areas();

                            areas
                                .visible_layer_ids()
                                .into_iter()
                                .filter(|layer| layer.order > egui::Order::Background)
                                .filter(|layer| areas.is_visible(layer))
                                .map(|layer| layer.id)
                                .collect::<Vec<_>>()
                        })
                        .into_iter()
                        .filter_map(|id| AreaState::load(&self.egui_context, id));

                    for layer in layers {
                        if let (Some(pos), Some(size)) = (layer.pivot_pos, layer.size) {
                            region.add(
                                pos.x.floor() as i32,
                                pos.y.floor() as i32,
                                size.x.ceil() as i32,
                                size.y.ceil() as i32,
                            );
                        }
                    }

                    self.layer.set_input_region(Some(region.wl_region()));
                }
            }
            InputRegions::None => {
                if let Ok(region) = Region::new(compositor) {
                    region.add(0, 0, 0, 0);
                    self.layer.set_input_region(Some(region.wl_region()));
                }
            }
        }

        // if self.egui_context.wants_pointer_input() {
        //     self.layer.set_input_region(None);
        // } else if let Ok(region) = Region::new(compositor) {
        //     region.add(0, 0, 0, 0);
        //     self.layer.set_input_region(Some(region.wl_region()));
        // }

        // Submit the command in the queue to execute
        queue.submit(Some(encoder.finish()));

        surface_texture.present();
    }
}

impl CompositorHandler for ContextDelegate {
    // this is only for integer scaling
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        if let Some(app) = self.apps.get_mut(&surface.id()) {
            if app.scale.round() != app.scale {
                // app is already fractionally scaled?
                // TODO: is this ok?
                return;
            }

            self.scale_factor_changed(qh, surface, new_factor as f32);
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
        // TODO
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        if let Some(app) = self.apps.get_mut(&surface.id()) {
            app.draw(&self.compositor);
        }
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
        // TODO
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
        // TODO
    }
}

impl OutputHandler for ContextDelegate {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        // TODO
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        // TODO
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        // TODO
    }
}

impl LayerShellHandler for ContextDelegate {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        // TODO
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if let Some(app) = self.apps.get_mut(&layer.wl_surface().id()) {
            if configure.new_size.0 == 0 || configure.new_size.1 == 0 {
                app.width = DEFAULT_WIDTH;
                app.height = DEFAULT_HEIGHT;
            } else {
                app.width = configure.new_size.0;
                app.height = configure.new_size.1;
            }

            // let surface_format = app
            //     .wgpu_surface
            //     .get_supported_formats(&app.egui_render_state.adapter)[0];

            // let mut surface_config = wgpu::SurfaceConfiguration {
            //     usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            //     format: TextureFormat::Bgra8UnormSrgb,
            //     width: app.width,
            //     height: app.height,
            //     present_mode: wgpu::PresentMode::Fifo,
            //     alpha_mode: CompositeAlphaMode::Auto,
            //     desired_maximum_frame_latency
            // };
            // surface.configure(&device, &surface_config);

            // Initiate the first draw.
            if app.first_configure {
                app.first_configure = false;
                app.draw(&self.compositor);
            }
        }
    }
}

impl SeatHandler for ContextDelegate {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            println!("Set keyboard capability");
            let keyboard = self
                .seat_state
                .get_keyboard(qh, &seat, None)
                .expect("Failed to create keyboard");
            self.keyboard = Some(keyboard);
        }

        if capability == Capability::Pointer && self.pointer.is_none() {
            println!("Set pointer capability");
            let pointer = self
                .seat_state
                .get_pointer(qh, &seat)
                .expect("Failed to create pointer");
            self.pointer = Some(pointer);
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_some() {
            println!("Unset keyboard capability");
            self.keyboard.take().unwrap().release();
        }

        if capability == Capability::Pointer && self.pointer.is_some() {
            println!("Unset pointer capability");
            self.pointer.take().unwrap().release();
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for ContextDelegate {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
        if let Some(app) = self.apps.get_mut(&surface.id()) {
            app.keyboard_focus = true;
            // app.modifiers = Modifiers::from_keysyms(keysyms);
            // app.draw(&self.event_queue.handle());
        }
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
        // if self.layer.wl_surface() == surface {
        //     println!("Release keyboard focus on window");
        //     self.keyboard_focus = false;
        // }
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        println!("Key press: {event:?}");
        // press 'esc' to exit
        if event.keysym == Keysym::Escape {
            // self.exit = true;
        }
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        println!("Key release: {event:?}");
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        modifiers: Modifiers,
        _layout: u32,
    ) {
        println!("Update modifiers: {modifiers:?}");
    }
}

impl PointerHandler for ContextDelegate {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for PointerEvent { surface, position, kind } in events {
            if let Some(app) = self.apps.get_mut(&surface.id()) {
                app.egui_context.request_repaint();

                let pos = egui::pos2(position.0 as f32, position.1 as f32);
                let ev = match kind {
                    PointerEventKind::Enter { .. } => egui::Event::PointerMoved(pos),
                    PointerEventKind::Leave { .. } => egui::Event::PointerGone,
                    PointerEventKind::Motion { .. } => egui::Event::PointerMoved(pos),
                    PointerEventKind::Axis { horizontal, vertical, .. } => {
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Line,
                            delta: egui::vec2(
                                horizontal.discrete as f32,
                                -vertical.discrete as f32,
                            ),
                            modifiers: app.modifiers,
                        }
                    }
                    PointerEventKind::Press { button, .. }
                    | PointerEventKind::Release { button, .. } => {
                        use smithay_client_toolkit::seat::pointer::*;
                        egui::Event::PointerButton {
                            pos,
                            button: match *button {
                                BTN_RIGHT => egui::PointerButton::Secondary,
                                BTN_MIDDLE => egui::PointerButton::Middle,
                                BTN_BACK | BTN_SIDE => egui::PointerButton::Extra1,
                                BTN_FORWARD | BTN_EXTRA => egui::PointerButton::Extra2,
                                _ => egui::PointerButton::Primary, // BTN_LEFT and unknown
                            },
                            pressed: matches!(kind, PointerEventKind::Press { .. }),
                            modifiers: app.modifiers,
                        }
                    }
                };

                app.events.push(ev);

                app.egui_context.request_repaint();
            }
        }
    }
}

delegate_compositor!(ContextDelegate);
delegate_output!(ContextDelegate);

delegate_seat!(ContextDelegate);
delegate_keyboard!(ContextDelegate);
delegate_pointer!(ContextDelegate);

delegate_layer!(ContextDelegate);

delegate_registry!(ContextDelegate);

impl ProvidesRegistryState for ContextDelegate {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}
