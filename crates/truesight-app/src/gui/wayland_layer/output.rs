use super::super::overlay_backend::LogicalOutputBounds;
use super::{LayerEvent, WaylandLayerState};
use smithay_client_toolkit::reexports::client::protocol::wl_output;

pub(super) struct TargetOutputLayout {
    pub(super) id: Option<u32>,
    pub(super) output: Option<wl_output::WlOutput>,
    pub(super) origin: (i32, i32),
    pub(super) size: Option<(i32, i32)>,
    pub(super) scale: i32,
}

/// Builds a `LayerEvent::MonitorsUpdated` snapshot excluding a dying output (SCTK 0.20 calls
/// `output_destroyed` before pruning `OutputState`). Reports `has_any_output` independently of
/// optional `xdg-output` logical rectangles so the GUI waits at zero outputs and still recovers
/// when a `wl_output` returns without logical bounds.
pub(in crate::gui) fn build_monitors_updated_event(
    all_output_ids: &[u32],
    logical_outputs: Vec<LogicalOutputBounds>,
    destroyed_output_id: Option<u32>,
) -> LayerEvent {
    let has_any_output = all_output_ids
        .iter()
        .any(|&id| destroyed_output_id != Some(id));
    let outputs = logical_outputs
        .into_iter()
        .filter(|out| destroyed_output_id != Some(out.id))
        .collect();
    LayerEvent::MonitorsUpdated {
        outputs,
        has_any_output,
    }
}

impl WaylandLayerState {
    pub(super) fn emit_monitors(&self) {
        self.emit_monitors_excluding(None);
    }

    pub(super) fn emit_monitors_excluding(&self, destroyed_output: Option<&wl_output::WlOutput>) {
        let destroyed_id = destroyed_output
            .and_then(|out| self.output_state.info(out))
            .map(|info| info.id);
        let all_ids: Vec<u32> = self
            .output_state
            .outputs()
            .filter(|o| destroyed_output.is_none_or(|dead| o != dead))
            .map(|o| self.output_state.info(&o).map_or(0, |i| i.id))
            .collect();
        self.emit(build_monitors_updated_event(
            &all_ids,
            self.logical_outputs(),
            destroyed_id,
        ));
    }

    pub(super) fn logical_outputs(&self) -> Vec<LogicalOutputBounds> {
        self.output_state
            .outputs()
            .filter_map(|o| {
                let info = self.output_state.info(&o)?;
                let (lw, lh) = info.logical_size.filter(|&(w, h)| w > 0 && h > 0)?;
                let (lx, ly) = info.logical_position.unwrap_or((0, 0));
                Some(LogicalOutputBounds {
                    id: info.id,
                    x: lx,
                    y: ly,
                    width: lw,
                    height: lh,
                })
            })
            .collect()
    }

    pub(super) fn output_by_id(&self, id: u32) -> Option<(wl_output::WlOutput, i32)> {
        for out in self.output_state.outputs() {
            if let Some(info) = self.output_state.info(&out)
                && info.id == id
            {
                return Some((out, info.scale_factor.max(1)));
            }
        }
        None
    }

    pub(super) fn resolve_target_output(
        &self,
        desktop_x: i32,
        desktop_y: i32,
        width: u32,
        height: u32,
    ) -> TargetOutputLayout {
        let cx = desktop_x + (width as i32 / 2);
        let cy = desktop_y + (height as i32 / 2);
        let mut fallback = None;

        for out in self.output_state.outputs() {
            let Some(info) = self.output_state.info(&out) else {
                continue;
            };
            let origin = info.logical_position.unwrap_or((0, 0));
            let size = info.logical_size.filter(|&(w, h)| w > 0 && h > 0);
            let scale = info.scale_factor.max(1);
            if fallback.is_none() {
                fallback = Some(TargetOutputLayout {
                    id: Some(info.id),
                    output: Some(out.clone()),
                    origin,
                    size,
                    scale,
                });
            }
            if let Some((lw, lh)) = size
                && cx >= origin.0
                && cx < origin.0 + lw
                && cy >= origin.1
                && cy < origin.1 + lh
            {
                return TargetOutputLayout {
                    id: Some(info.id),
                    output: Some(out),
                    origin,
                    size: Some((lw, lh)),
                    scale,
                };
            }
        }

        fallback.unwrap_or(TargetOutputLayout {
            id: None,
            output: None,
            origin: (0, 0),
            size: None,
            scale: 1,
        })
    }
}
