//! Handling of the fractional scaling.

use smithay_client_toolkit::reexports::client::globals::{BindError, GlobalList};
use smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface;
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, Proxy, QueueHandle, delegate_dispatch};
use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    Event as FractionalScalingEvent, WpFractionalScaleV1,
};

use smithay_client_toolkit::globals::GlobalData;

use super::ContextDelegate;

const SCALE_DENOMINATOR: f32 = 120.;

#[derive(Debug)]
pub struct FractionalScalingManager {
    manager: WpFractionalScaleManagerV1,
}

pub struct FractionalScaling {
    surface: WlSurface,
}

impl FractionalScalingManager {
    pub fn bind(
        globals: &GlobalList,
        queue_handle: &QueueHandle<ContextDelegate>,
    ) -> Result<Self, BindError> {
        let manager = globals.bind(queue_handle, 1..=1, GlobalData)?;

        Ok(Self { manager })
    }

    pub fn fractional_scaling(
        &self,
        surface: &WlSurface,
        queue_handle: &QueueHandle<ContextDelegate>,
    ) -> WpFractionalScaleV1 {
        let data = FractionalScaling { surface: surface.clone() };

        self.manager
            .get_fractional_scale(surface, queue_handle, data)
    }
}

impl Dispatch<WpFractionalScaleManagerV1, GlobalData, ContextDelegate>
    for FractionalScalingManager
{
    fn event(
        _: &mut ContextDelegate,
        _: &WpFractionalScaleManagerV1,
        _: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<ContextDelegate>,
    ) {
        // No events.
    }
}

impl Dispatch<WpFractionalScaleV1, FractionalScaling, ContextDelegate>
    for FractionalScalingManager
{
    fn event(
        state: &mut ContextDelegate,
        _: &WpFractionalScaleV1,
        event: <WpFractionalScaleV1 as Proxy>::Event,
        data: &FractionalScaling,
        _: &Connection,
        qh: &QueueHandle<ContextDelegate>,
    ) {
        if let FractionalScalingEvent::PreferredScale { scale } = event {
            state.scale_factor_changed(qh, &data.surface, scale as f32 / SCALE_DENOMINATOR);
        }
    }
}

delegate_dispatch!(ContextDelegate: [WpFractionalScaleManagerV1: GlobalData] => FractionalScalingManager);
delegate_dispatch!(ContextDelegate: [WpFractionalScaleV1: FractionalScaling] => FractionalScalingManager);
