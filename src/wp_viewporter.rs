use smithay_client_toolkit::globals::GlobalData;
use smithay_client_toolkit::reexports::client::globals::{BindError, GlobalList};
use smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface;
use smithay_client_toolkit::reexports::client::{
    Connection, Dispatch, Proxy, QueueHandle, delegate_dispatch,
};
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport;
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use super::ContextDelegate;

/// Viewporter.
#[derive(Debug)]
pub struct ViewporterState {
    viewporter: WpViewporter,
}

impl ViewporterState {
    /// Create new viewporter.
    pub fn bind(
        globals: &GlobalList,
        queue_handle: &QueueHandle<ContextDelegate>,
    ) -> Result<Self, BindError> {
        let viewporter = globals.bind(queue_handle, 1..=1, GlobalData)?;
        Ok(Self { viewporter })
    }

    /// Get the viewport for the given object.
    pub fn get_viewport(
        &self,
        surface: &WlSurface,
        queue_handle: &QueueHandle<ContextDelegate>,
    ) -> WpViewport {
        self.viewporter
            .get_viewport(surface, queue_handle, GlobalData)
    }
}

impl Dispatch<WpViewporter, GlobalData, ContextDelegate> for ViewporterState {
    fn event(
        _: &mut ContextDelegate,
        _: &WpViewporter,
        _: <WpViewporter as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<ContextDelegate>,
    ) {
        // No events.
    }
}
impl Dispatch<WpViewport, GlobalData, ContextDelegate> for ViewporterState {
    fn event(
        _: &mut ContextDelegate,
        _: &WpViewport,
        _: <WpViewport as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<ContextDelegate>,
    ) {
        // No events.
    }
}

delegate_dispatch!(ContextDelegate: [WpViewporter: GlobalData] => ViewporterState);
delegate_dispatch!(ContextDelegate: [WpViewport: GlobalData] => ViewporterState);
