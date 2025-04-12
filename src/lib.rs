use std::{collections::HashMap, io::ErrorKind, ptr::NonNull, time::Instant};

use egui_wgpu::{ScreenDescriptor, WgpuConfiguration, wgpu::TextureFormat};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
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
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
};
use wayland_backend::client::{ObjectId, WaylandError};
use wayland_client::{
    Connection, DispatchError, EventQueue, Proxy as _, QueueHandle, delegate_dispatch,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface},
};
use wgpu::{
    CompositeAlphaMode,
    rwh::{RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle},
};

use self::{wp_fractional_scaling::FractionalScalingManager, wp_viewporter::ViewporterState};

mod wp_fractional_scaling;
mod wp_viewporter;

const DEFAULT_WIDTH: u32 = 512;
const DEFAULT_HEIGHT: u32 = 512;

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

            app.scale = new_factor;
            app.draw(qh);

            let viewport = self.viewporter.get_viewport(app.layer.wl_surface(), qh);
            viewport.set_destination(app.width as i32, app.height as i32);
        }
    }
}

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

    pub fn new_layer_app(&mut self, app: Box<dyn App + Send>) {
        let qh = self.event_queue.handle();

        // A layer surface is created from a surface.
        let wl_surface = self.delegate.compositor.create_surface(&qh);

        // And then we create the layer shell.
        let layer = self.delegate.layer_shell.create_layer_surface(
            &qh,
            wl_surface,
            Layer::Top,
            Some("simple_layer"),
            None,
        );

        // let viewport = layer

        // Configure the layer surface, providing things like the anchor on screen, desired size and the keyboard
        // interactivity
        // TODO: make user-configurable
        layer.set_anchor(Anchor::TOP | Anchor::RIGHT);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(DEFAULT_WIDTH, DEFAULT_HEIGHT);
        layer.set_exclusive_zone(8);

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

        let msaa_samples = 1;
        let dithering = true;

        // dbg!(wgpu_surface.

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

    pub fn poll_events(&mut self) -> Result<usize, DispatchError> {
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
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

pub trait App {
    fn update(&mut self, ctx: &egui::Context);
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
    fractional_scale: WpFractionalScaleV1,

    start: Instant,
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

    fn draw(&mut self, qh: &QueueHandle<ContextDelegate>) {
        // TODO: input
        let raw_input = egui::RawInput {
            time: Some(self.start.elapsed().as_secs_f64()),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0., 0.),
                egui::vec2(self.width as f32, self.height as f32),
            )),
            ..Default::default()
        };

        self.egui_context.set_pixels_per_point(self.scale);

        let surface = self.layer.wl_surface().clone();
        let qh = qh.clone();
        self.egui_context.set_request_repaint_callback(move |info| {
            surface.frame(&qh, surface.clone());
        });

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
            alpha_mode: CompositeAlphaMode::Auto,
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
            size_in_pixels: [self.physical_height(), self.physical_height()],
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
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLUE),
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
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        if let Some(app) = self.apps.get_mut(&surface.id()) {
            app.draw(qh);
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
        qh: &QueueHandle<Self>,
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
                app.draw(qh);
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
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        keysyms: &[Keysym],
    ) {
        // if self.layer.wl_surface() == surface {
        //     println!("Keyboard focus on window with pressed syms: {keysyms:?}");
        //     self.keyboard_focus = true;
        // }
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
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
        _: &wl_keyboard::WlKeyboard,
        _: u32,
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
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        println!("Key release: {event:?}");
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
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
        use PointerEventKind::*;
        for event in events {
            // Ignore events for other surfaces
            // if &event.surface != self.layer.wl_surface() {
            //     continue;
            // }
            match event.kind {
                Enter { .. } => {
                    println!("Pointer entered @{:?}", event.position);
                }
                Leave { .. } => {
                    println!("Pointer left");
                }
                Motion { .. } => {}
                Press { button, .. } => {
                    println!("Press {:x} @ {:?}", button, event.position);
                    // self.shift = self.shift.xor(Some(0));
                }
                Release { button, .. } => {
                    println!("Release {:x} @ {:?}", button, event.position);
                }
                Axis { horizontal, vertical, .. } => {
                    println!("Scroll H:{horizontal:?}, V:{vertical:?}");
                }
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
