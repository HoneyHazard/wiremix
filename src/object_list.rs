//! A Ratatui widget for an interactable list of PipeWire objects.

use std::cmp;
use std::collections::HashSet;

use ratatui::{
    prelude::{Alignment, Buffer, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{ListState, StatefulWidget, Widget},
};

use crossterm::event::{MouseButton, MouseEventKind};
use smallvec::smallvec;

use crate::app::{Action, MouseArea};
use crate::config::Config;
use crate::device_kind::DeviceKind;
use crate::device_widget::DeviceWidget;
use crate::dropdown_widget::DropdownWidget;
use crate::node_widget::NodeWidget;
use crate::view::{self, ListKind, VolumeAdjustment};
use crate::wirehose::ObjectId;

/// ObjectList stores information for filtering and displaying a subset of
/// objects from a [`View`](`crate::view::View`).
///
/// Control operations pertaining to individual objects are handled here.
#[derive(Default)]
pub struct ObjectList {
    /// Index of the first object in viewport
    top: usize,
    /// ID of the currently selected object
    pub selected: Option<ObjectId>,
    /// Which set of objects to use from the View
    list_kind: ListKind,
    /// Default device type to use for defaults and node rendering
    device_kind: Option<DeviceKind>,
    /// Target dropdown state
    pub dropdown_state: ListState,
    /// Targets
    pub targets: Vec<(view::Target, String)>,
    /// Number of objects visible at once, as of the last `update()` call.
    /// Cached here (rather than threaded into every action handler) since
    /// it only changes on terminal resize and `update()` already recomputes
    /// it every frame from the current area.
    page_size: usize,
}

impl ObjectList {
    pub fn new(list_kind: ListKind, device_kind: Option<DeviceKind>) -> Self {
        Self {
            top: 0,
            selected: None,
            list_kind,
            device_kind,
            ..Default::default()
        }
    }

    pub fn down(&mut self, view: &view::View) {
        if self.dropdown_state.selected().is_some() {
            self.dropdown_state.select_next();
        } else {
            let new_selected = view.next_id(self.list_kind, self.selected);
            if new_selected.is_some() {
                self.select(new_selected);
            }
        }
    }

    pub fn up(&mut self, view: &view::View) {
        if self.dropdown_state.selected().is_some() {
            self.dropdown_state.select_previous();
        } else {
            let new_selected = view.previous_id(self.list_kind, self.selected);
            if new_selected.is_some() {
                self.select(new_selected);
            }
        }
    }

    /// Move the selection down by a page (however many objects are visible
    /// at once), or to the last object if fewer than a page remain. Leaves
    /// the dropdown selection to select_next() the same as `down()`, since
    /// a "page" of dropdown targets isn't a meaningful concept - dropdowns
    /// are always small.
    pub fn page_down(&mut self, view: &view::View) {
        if self.dropdown_state.selected().is_some() {
            self.dropdown_state.select_next();
            return;
        }
        let mut new_selected = self.selected;
        for _ in 0..self.page_size.max(1) {
            match view.next_id(self.list_kind, new_selected) {
                Some(id) => new_selected = Some(id),
                None => break,
            }
        }
        if new_selected.is_some() {
            self.select(new_selected);
        }
    }

    /// Move the selection up by a page. See `page_down()`.
    pub fn page_up(&mut self, view: &view::View) {
        if self.dropdown_state.selected().is_some() {
            self.dropdown_state.select_previous();
            return;
        }
        let mut new_selected = self.selected;
        for _ in 0..self.page_size.max(1) {
            match view.previous_id(self.list_kind, new_selected) {
                Some(id) => new_selected = Some(id),
                None => break,
            }
        }
        if new_selected.is_some() {
            self.select(new_selected);
        }
    }

    /// Jump the selection straight to the first object in the list.
    pub fn first(&mut self, view: &view::View) {
        if self.dropdown_state.selected().is_some() {
            self.dropdown_state.select_first();
            return;
        }
        if let Some(&id) = view.object_ids(self.list_kind).first() {
            self.select(Some(id));
        }
    }

    /// Jump the selection straight to the last object in the list.
    pub fn last(&mut self, view: &view::View) {
        if self.dropdown_state.selected().is_some() {
            self.dropdown_state.select_last();
            return;
        }
        if let Some(&id) = view.object_ids(self.list_kind).last() {
            self.select(Some(id));
        }
    }

    fn dropdown_open(&mut self, view: &view::View) {
        let targets = match self.list_kind {
            ListKind::Node(_) => self
                .selected
                .and_then(|object_id| view.node_targets(object_id)),
            ListKind::Device => self
                .selected
                .and_then(|object_id| view.device_targets(object_id)),
        };
        if let Some((targets, index)) = targets {
            if !targets.is_empty() {
                self.targets = targets;
                self.dropdown_state.select(Some(index));
            }
        }
    }

    fn selected_target(&self) -> Option<&view::Target> {
        self.dropdown_state
            .selected()
            .and_then(|index| self.targets.get(index))
            .map(|(target, _)| target)
    }

    pub fn dropdown_activate(&mut self, view: &view::View) {
        // Just open the dropdown if it's not showing yet.
        if self.dropdown_state.selected().is_none() {
            self.dropdown_open(view);
            return;
        }

        if let (Some(object_id), Some(&target)) =
            (self.selected, self.selected_target())
        {
            view.set_target(object_id, target);
        };

        self.dropdown_state.select(None);
    }

    pub fn dropdown_close(&mut self) {
        self.dropdown_state.select(None);
    }

    pub fn set_target(&mut self, view: &view::View, target: view::Target) {
        self.dropdown_state.select(None);
        if let Some(object_id) = self.selected {
            view.set_target(object_id, target);
        };
    }

    pub fn toggle_mute(&mut self, view: &view::View) {
        if matches!(self.list_kind, ListKind::Device) {
            return;
        }
        if let Some(node_id) = self.selected {
            view.mute(node_id);
        }
    }

    pub fn set_absolute_volume(
        &mut self,
        view: &view::View,
        volume: f32,
        max: Option<f32>,
    ) -> bool {
        if matches!(self.list_kind, ListKind::Device) {
            return false;
        }
        if let Some(node_id) = self.selected {
            return view.volume(
                node_id,
                VolumeAdjustment::Absolute(volume),
                max,
            );
        }
        false
    }

    pub fn set_relative_volume(
        &mut self,
        view: &view::View,
        volume: f32,
        max: Option<f32>,
    ) -> bool {
        if matches!(self.list_kind, ListKind::Device) {
            return false;
        }
        if let Some(node_id) = self.selected {
            return view.volume(
                node_id,
                VolumeAdjustment::Relative(volume),
                max,
            );
        }
        false
    }

    pub fn set_default(&mut self, view: &view::View) {
        if matches!(self.list_kind, ListKind::Device) {
            return;
        }
        if let Some(node_id) = self.selected {
            if let Some(device_kind) = self.device_kind {
                view.set_default(node_id, device_kind);
            } else {
                view.set_target(node_id, view::Target::Default);
            }
        }
    }

    fn selected_index(&self, view: &view::View) -> Option<usize> {
        self.selected
            .and_then(|selected| view.position(self.list_kind, selected))
    }

    fn select(&mut self, object_id: Option<ObjectId>) {
        self.selected = object_id;
        // Close the dropdown in case it is open for the previously-selected
        // object. This can happen when the object is removed from PipeWire
        // while the dropdown is open.
        self.dropdown_close();
    }

    /// Returns a set of object IDs of the visible objects. This includes all
    /// dependencies that affect the display of the objects.
    pub fn visible_objects(
        &self,
        area: &Rect,
        view: &view::View,
    ) -> HashSet<ObjectId> {
        let objects = view.object_ids(self.list_kind);

        let last = cmp::min(objects.len(), self.top + self.visible_count(area));

        // Always include object 0 - the global PipeWire state.
        let mut visible_objects = HashSet::from([ObjectId::from_raw_id(0)]);

        for object_id in objects[self.top..last].iter().cloned() {
            visible_objects.insert(object_id);
            if let Some(node) = view.nodes.get(&object_id) {
                // Add linked client and device.
                visible_objects.extend(node.client_id);
                visible_objects.extend(node.device_info.map(|(id, _, _)| id));

                // Add the target and any linked client and device.
                if let ListKind::Node(node_kind) = self.list_kind {
                    if let Some(target_id) = node
                        .target
                        .and_then(|target| target.resolve(view, node_kind))
                    {
                        visible_objects.insert(target_id);
                        if let Some(target_node) = view.nodes.get(&target_id) {
                            visible_objects.extend(target_node.client_id);
                            visible_objects.extend(
                                target_node.device_info.map(|(id, _, _)| id),
                            );
                        }
                    }
                }
            }
        }

        visible_objects
    }

    /// Returns the number of objects visible.
    fn visible_count(&self, area: &Rect) -> usize {
        let (_, list_area, _) = self.areas(area);
        let full_height = match self.list_kind {
            ListKind::Node(_) => {
                NodeWidget::height().saturating_add(NodeWidget::spacing())
            }
            ListKind::Device => {
                DeviceWidget::height().saturating_add(DeviceWidget::spacing())
            }
        };
        (list_area.height / full_height) as usize
    }

    /// Reconciles changes to objects, viewport, and selection.
    pub fn update(&mut self, area: Rect, view: &view::View) {
        let selected_index = self.selected_index(view).or_else(|| {
            // There's nothing selected! Select the first item and try again.
            self.select(view.next_id(self.list_kind, None));
            self.selected_index(view)
        });

        let objects_len = view.len(self.list_kind);

        let visible_count = self.visible_count(&area);
        self.page_size = visible_count;

        // If objects were removed and the viewport is now below the visible
        // objects, move the viewport up so that the bottom of the object list
        // is visible.
        if self.top >= objects_len {
            self.top = objects_len.saturating_sub(visible_count);
        }

        // Make sure the selected object is visible and adjust the viewport
        // if necessary.
        if self.selected.is_some() {
            match selected_index {
                Some(selected_index) => {
                    if selected_index >= self.top.saturating_add(visible_count)
                    {
                        // The selection is below the viewport. Reposition the
                        // viewport so that the selected item is at the bottom.
                        let visible_count_except_last =
                            visible_count.saturating_sub(1);
                        self.top = selected_index
                            .saturating_sub(visible_count_except_last);
                    } else if selected_index < self.top {
                        // The selected item is above the viewport. Reposition
                        // so that it's the first visible item.
                        self.top = selected_index;
                    }
                }
                None => self.select(None), // The selected object is gone!
            }
        }
    }

    fn areas(&self, area: &Rect) -> (Rect, Rect, Rect) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header_area
                Constraint::Min(0),    // list_area
                Constraint::Length(1), // footer_area
            ])
            .split(*area);

        (layout[0], layout[1], layout[2])
    }
}

pub struct ObjectListWidget<'a, 'b> {
    pub object_list: &'a mut ObjectList,
    pub view: &'a view::View<'b>,
    pub config: &'a Config,
}

struct ObjectListRenderContext<'a> {
    header_area: Rect,
    list_area: Rect,
    footer_area: Rect,
    objects_layout: &'a [Rect],
    objects_visible: usize,
}

/// Extends a selected row's `row_selected` background one row above
/// and/or one row below it (independently, per `row_selected_extend_above`/
/// `row_selected_extend_below`), so the highlight doesn't cut off abruptly
/// right at the row's own edges. Clipped to `above_clip`/`below_clip`
/// respectively so it can never bleed into a neighboring item.
///
/// For most objects both clip areas are just `list_area`, but the very
/// first/last object in the whole list has no list row of its own to
/// extend into on that side - the row directly above the first object (or
/// below the last) is `header_area`/`footer_area`, a different `Rect`
/// reserved for the scroll indicator. Callers pass a clip area that
/// includes the header/footer specifically for that edge object, but only
/// when the corresponding scroll indicator isn't being drawn there (see
/// the call sites in `render_node_list`/`render_device_list`) - otherwise
/// the highlight would paint over the "more items" indicator.
///
/// Also extends the selector marker (the left gutter column) to match,
/// reusing the `selector_middle` glyph for the extra row - but only when
/// `row_selected` is actually customized away from the default empty
/// `{ }`. Drawing extra marker glyphs is real cell content, not a style
/// patch, so unlike the background fill (where an empty Style is
/// naturally a no-op via `Cell::set_style`'s `Some(..)`-only overwrite),
/// it can't rely on being inert by default - it needs its own explicit
/// check so the marker's height stays exactly what it's always been for
/// every theme that doesn't opt into `row_selected`.
///
/// Both `row_selected_extend_above`/`_below` default to `false`, so this
/// whole function is a no-op - no size or height change to the marker or
/// background - unless a config explicitly turns one or both on.
fn extend_selected_row(
    buf: &mut Buffer,
    config: &Config,
    object_area: Rect,
    above_clip: Rect,
    below_clip: Rect,
    spacing: u16,
) {
    let max_extend = spacing.min(1);

    let extend_one_side = |buf: &mut Buffer, y: u16, clip: Rect| {
        let extend_area = Rect {
            x: object_area.x,
            y,
            width: object_area.width,
            height: max_extend,
        };
        let clipped = clip.intersection(extend_area);
        buf.set_style(clipped, config.theme.row_selected);

        if config.theme.row_selected == Style::default() {
            return;
        }

        let marker_area = Rect {
            width: 1,
            ..extend_area
        };
        Line::from(Span::styled(
            &config.char_set.selector_middle,
            config.theme.selector,
        ))
        .render(clip.intersection(marker_area), buf);
    };

    if config.row_selected_extend_above {
        extend_one_side(
            buf,
            object_area.y.saturating_sub(max_extend),
            above_clip,
        );
    }

    if config.row_selected_extend_below {
        extend_one_side(
            buf,
            object_area.y.saturating_add(object_area.height),
            below_clip,
        );
    }
}

/// Draws a one-row divider line spanning `object_area`'s width, in the gap
/// immediately below it, when `config.show_dividers` is set. A no-op
/// (nothing drawn, no space reserved) when it isn't - dividers only ever
/// use space that the existing gap between items (NodeWidget::spacing()/
/// DeviceWidget::spacing()) already reserves, so turning this on never
/// changes the list's layout, only what's drawn in space that was already
/// blank. Clipped to `list_area` so it can never bleed into a neighboring
/// item or past the list into the footer/tab bar.
fn render_divider(
    buf: &mut Buffer,
    config: &Config,
    object_area: Rect,
    list_area: Rect,
) {
    if !config.show_dividers {
        return;
    }

    let divider_area = Rect {
        x: object_area.x,
        y: object_area.y.saturating_add(object_area.height),
        width: object_area.width,
        height: 1,
    };
    let clipped = list_area.intersection(divider_area);
    if clipped.is_empty() {
        return;
    }

    let line = config.char_set.divider.repeat(clipped.width as usize);
    Line::from(Span::styled(line, config.theme.divider)).render(clipped, buf);
}

impl ObjectListWidget<'_, '_> {
    fn render_node_list(
        &mut self,
        node_kind: view::NodeKind,
        context: ObjectListRenderContext,
        area: Rect,
        buf: &mut Buffer,
        mouse_areas: &mut Vec<MouseArea>,
    ) {
        let all_objects = self.view.full_nodes(node_kind);
        let total_objects = all_objects.len();
        let objects = all_objects
            .iter()
            .skip(self.object_list.top)
            // Take one extra so we can render a partial node at the bottom of
            // the area.
            .take(context.objects_visible.saturating_add(1));

        let objects_and_areas: Vec<(&&view::Node, &Rect)> =
            objects.zip(context.objects_layout.iter()).collect();
        let last_index = objects_and_areas.len().saturating_sub(1);
        for (i, (object, &object_area)) in objects_and_areas.iter().enumerate()
        {
            let selected = self
                .object_list
                .selected
                .map(|id| id == object.object_id)
                .unwrap_or_default();
            NodeWidget::new(
                self.config,
                self.object_list.device_kind,
                object,
                selected,
            )
            .render(object_area, buf, mouse_areas);

            if selected {
                // No scroll-up indicator competing for header_area when
                // this is truly the first object in the list (not just
                // the first one currently visible) - safe to extend into
                // it. Same idea for footer_area/the last object below.
                let above_clip = if i == 0 && self.object_list.top == 0 {
                    context.header_area.union(context.list_area)
                } else {
                    context.list_area
                };
                let below_clip =
                    if self.object_list.top + i + 1 == total_objects {
                        context.list_area.union(context.footer_area)
                    } else {
                        context.list_area
                    };
                extend_selected_row(
                    buf,
                    self.config,
                    object_area,
                    above_clip,
                    below_clip,
                    NodeWidget::spacing(),
                );
            }

            if i < last_index {
                render_divider(buf, self.config, object_area, context.list_area);
            }
        }

        // Show the target dropdown?
        if self.object_list.dropdown_state.selected().is_some() {
            // Get the area for the selected object
            if let Some((_, object_area)) =
                objects_and_areas.iter().find(|(object, _)| {
                    self.object_list
                        .selected
                        .map(|id| id == object.object_id)
                        .unwrap_or_default()
                })
            {
                DropdownWidget::new(
                    self.object_list,
                    &NodeWidget::dropdown_area(
                        self.object_list,
                        &context.list_area,
                        object_area,
                    ),
                    self.config,
                )
                .render(area, buf, mouse_areas);
            }
        }
    }

    fn render_device_list(
        &mut self,
        context: ObjectListRenderContext,
        area: Rect,
        buf: &mut Buffer,
        mouse_areas: &mut Vec<MouseArea>,
    ) {
        let all_objects = self.view.full_devices();
        let total_objects = all_objects.len();
        let objects = all_objects
            .iter()
            .skip(self.object_list.top)
            // Take one extra so we can render a partial node at the bottom of
            // the area.
            .take(context.objects_visible.saturating_add(1));

        let objects_and_areas: Vec<(&&view::Device, &Rect)> =
            objects.zip(context.objects_layout.iter()).collect();
        let last_index = objects_and_areas.len().saturating_sub(1);
        for (i, (object, &object_area)) in objects_and_areas.iter().enumerate()
        {
            let selected = self
                .object_list
                .selected
                .map(|id| id == object.object_id)
                .unwrap_or_default();
            DeviceWidget::new(object, selected, self.config).render(
                object_area,
                buf,
                mouse_areas,
            );

            if selected {
                // See the matching comment in render_node_list().
                let above_clip = if i == 0 && self.object_list.top == 0 {
                    context.header_area.union(context.list_area)
                } else {
                    context.list_area
                };
                let below_clip =
                    if self.object_list.top + i + 1 == total_objects {
                        context.list_area.union(context.footer_area)
                    } else {
                        context.list_area
                    };
                extend_selected_row(
                    buf,
                    self.config,
                    object_area,
                    above_clip,
                    below_clip,
                    DeviceWidget::spacing(),
                );
            }

            if i < last_index {
                render_divider(buf, self.config, object_area, context.list_area);
            }
        }

        // Show the target dropdown?
        if self.object_list.dropdown_state.selected().is_some() {
            // Get the area for the selected object
            if let Some((_, object_area)) =
                objects_and_areas.iter().find(|(object, _)| {
                    self.object_list
                        .selected
                        .map(|id| id == object.object_id)
                        .unwrap_or_default()
                })
            {
                DropdownWidget::new(
                    self.object_list,
                    &DeviceWidget::dropdown_area(
                        self.object_list,
                        &context.list_area,
                        object_area,
                    ),
                    self.config,
                )
                .render(area, buf, mouse_areas);
            }
        }
    }
}

impl StatefulWidget for &mut ObjectListWidget<'_, '_> {
    type State = Vec<MouseArea>;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let mouse_areas = state;

        let (header_area, list_area, footer_area) =
            self.object_list.areas(&area);

        mouse_areas.push((
            header_area,
            smallvec![MouseEventKind::Down(MouseButton::Left)],
            smallvec![Action::MoveUp],
        ));

        mouse_areas.push((
            footer_area,
            smallvec![MouseEventKind::Down(MouseButton::Left)],
            smallvec![Action::MoveDown],
        ));

        mouse_areas.push((
            list_area,
            smallvec![MouseEventKind::ScrollUp],
            smallvec![Action::MoveUp],
        ));

        mouse_areas.push((
            list_area,
            smallvec![MouseEventKind::ScrollDown],
            smallvec![Action::MoveDown],
        ));

        let (spacing, height) = match self.object_list.list_kind {
            ListKind::Node(_) => (NodeWidget::spacing(), NodeWidget::height()),
            ListKind::Device => {
                (DeviceWidget::spacing(), DeviceWidget::height())
            }
        };

        let full_object_height = height.saturating_add(spacing);
        let objects_visible = (list_area.height / full_object_height) as usize;

        let len = self.view.len(self.object_list.list_kind);

        // Indicate we can scroll up if there are objects above the viewport.
        if self.object_list.top > 0 {
            Line::from(Span::styled(
                &self.config.char_set.list_more,
                self.config.theme.list_more,
            ))
            .alignment(Alignment::Center)
            .render(header_area, buf);
        }

        // Indicate we can scroll down if there are objects below the
        // viewport, with an exception for when the last row is partially
        // rendered but still has all the important parts rendered,
        // excluding margins, etc.
        let is_bottom_last =
            self.object_list.top.saturating_add(objects_visible)
                == len.saturating_sub(1);
        let is_bottom_enough =
            (list_area.height % full_object_height) >= height;
        if self.object_list.top.saturating_add(objects_visible) < len
            && !(is_bottom_last && is_bottom_enough)
        {
            Line::from(Span::styled(
                &self.config.char_set.list_more,
                self.config.theme.list_more,
            ))
            .alignment(Alignment::Center)
            .render(footer_area, buf);
        }

        let objects_layout = {
            let object_height = height;
            let mut constraints =
                vec![Constraint::Length(object_height); objects_visible];
            // A variable-length constraint for a partial last object
            constraints.push(Constraint::Max(object_height));
            let constraints = constraints;

            Layout::default()
                .direction(Direction::Vertical)
                .constraints(constraints)
                .spacing(spacing)
                .split(list_area)
        };

        match self.object_list.list_kind {
            ListKind::Node(node_kind) => {
                self.render_node_list(
                    node_kind,
                    ObjectListRenderContext {
                        header_area,
                        list_area,
                        footer_area,
                        objects_layout: &objects_layout,
                        objects_visible,
                    },
                    area,
                    buf,
                    mouse_areas,
                );
            }
            ListKind::Device => {
                self.render_device_list(
                    ObjectListRenderContext {
                        header_area,
                        list_area,
                        footer_area,
                        objects_layout: &objects_layout,
                        objects_visible,
                    },
                    area,
                    buf,
                    mouse_areas,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use crate::mock;
    use crate::view::{ListKind, NodeKind, View};
    use crate::wirehose::{state::State, PropertyStore, StateEvent};
    use std::sync::Arc;

    fn init() -> (State, mock::WirehoseHandle<'static>) {
        let mut state = State::default();
        let wirehose = mock::WirehoseHandle::default();

        for i in 1..11 {
            let object_id = ObjectId::from_raw_id(i);
            let mut props = PropertyStore::default();
            props.set_node_description(String::from("Test node"));
            props.set_media_class(String::from("Stream/Output/Audio"));
            props.set_media_name(String::from("Media name"));
            props.set_node_name(String::from("Node name"));
            props.set_object_serial(i as u64);
            let props = props;

            let events = vec![
                StateEvent::NodeProperties { object_id, props },
                StateEvent::NodePositions {
                    object_id,
                    positions: vec![0, 1],
                },
                StateEvent::NodeStreamStarted {
                    object_id,
                    rate: 44100,
                    peaks: Arc::new([0.0.into(), 0.0.into()]),
                },
                StateEvent::NodeVolumes {
                    object_id,
                    volumes: vec![0.0, 0.0],
                },
                StateEvent::NodeMute {
                    object_id,
                    mute: false,
                },
            ];
            for event in events {
                state.update(event);
            }
        }

        (state, wirehose)
    }

    /// Helper to create a minimal node with the given media class.
    fn create_node(
        state: &mut State,
        object_id: ObjectId,
        media_class: &str,
        node_name: &str,
    ) {
        let mut props = PropertyStore::default();
        props.set_node_description(String::from("Test node"));
        props.set_media_class(String::from(media_class));
        props.set_media_name(String::from("Media name"));
        props.set_node_name(String::from(node_name));
        props.set_object_serial(u32::from(object_id) as u64);

        state.update(StateEvent::NodeProperties { object_id, props });
        state.update(StateEvent::NodeVolumes {
            object_id,
            volumes: vec![1.0, 1.0],
        });
        state.update(StateEvent::NodeMute {
            object_id,
            mute: false,
        });
    }

    #[test]
    fn object_list_up_overflow() {
        let (state, wirehose) = init();
        let view = View::from(
            &wirehose,
            &state,
            &config::Names::default(),
            &Vec::new(),
        );

        let height = NodeWidget::height() + NodeWidget::spacing();
        // + 2 for header and footer
        let rect = Rect::new(0, 0, 80, height * 3 + 2);
        let mut object_list =
            ObjectList::new(ListKind::Node(NodeKind::All), None);
        // Select first object
        object_list.down(&view);
        assert_eq!(object_list.top, 0);
        assert_eq!(object_list.selected, Some(ObjectId::from_raw_id(1)));

        object_list.up(&view);
        object_list.update(rect, &view);
        assert_eq!(object_list.top, 0);
        assert_eq!(object_list.selected, Some(ObjectId::from_raw_id(1)));
    }

    #[test]
    fn object_list_down_overflow() {
        let (state, wirehose) = init();
        let view = View::from(
            &wirehose,
            &state,
            &config::Names::default(),
            &Vec::new(),
        );

        let height = NodeWidget::height() + NodeWidget::spacing();
        // + 2 for header and footer
        let rect = Rect::new(0, 0, 80, height * 3 + 2);
        let mut object_list =
            ObjectList::new(ListKind::Node(NodeKind::All), None);
        // Select first object
        object_list.down(&view);
        assert_eq!(object_list.top, 0);
        assert_eq!(object_list.selected, Some(ObjectId::from_raw_id(1)));

        let nodes_len = view.nodes.len();

        for _ in 0..(nodes_len * 2) {
            object_list.down(&view);
        }

        object_list.update(rect, &view);
        assert_eq!(object_list.top, 7);
        assert_eq!(object_list.selected, Some(ObjectId::from_raw_id(10)));
    }

    #[test]
    fn object_list_page_down_page_up() {
        let (state, wirehose) = init();
        let view = View::from(
            &wirehose,
            &state,
            &config::Names::default(),
            &Vec::new(),
        );

        let height = NodeWidget::height() + NodeWidget::spacing();
        // + 2 for header and footer; 3 items visible at once
        let rect = Rect::new(0, 0, 80, height * 3 + 2);
        let mut object_list =
            ObjectList::new(ListKind::Node(NodeKind::All), None);

        // Select first object, and let update() compute page_size (3) from
        // the 3-item-tall rect above.
        object_list.down(&view);
        object_list.update(rect, &view);
        assert_eq!(object_list.selected, Some(ObjectId::from_raw_id(1)));

        object_list.page_down(&view);
        assert_eq!(object_list.selected, Some(ObjectId::from_raw_id(4)));

        object_list.page_up(&view);
        assert_eq!(object_list.selected, Some(ObjectId::from_raw_id(1)));
    }

    #[test]
    fn object_list_page_down_overflow() {
        let (state, wirehose) = init();
        let view = View::from(
            &wirehose,
            &state,
            &config::Names::default(),
            &Vec::new(),
        );

        let height = NodeWidget::height() + NodeWidget::spacing();
        let rect = Rect::new(0, 0, 80, height * 3 + 2);
        let mut object_list =
            ObjectList::new(ListKind::Node(NodeKind::All), None);

        object_list.down(&view);
        object_list.update(rect, &view);

        // Page down well past the last of the 10 mock nodes.
        for _ in 0..10 {
            object_list.page_down(&view);
            object_list.update(rect, &view);
        }
        assert_eq!(object_list.selected, Some(ObjectId::from_raw_id(10)));

        for _ in 0..10 {
            object_list.page_up(&view);
            object_list.update(rect, &view);
        }
        assert_eq!(object_list.selected, Some(ObjectId::from_raw_id(1)));
    }

    #[test]
    fn object_list_first_last() {
        let (state, wirehose) = init();
        let view = View::from(
            &wirehose,
            &state,
            &config::Names::default(),
            &Vec::new(),
        );

        let height = NodeWidget::height() + NodeWidget::spacing();
        let rect = Rect::new(0, 0, 80, height * 3 + 2);
        let mut object_list =
            ObjectList::new(ListKind::Node(NodeKind::All), None);

        // Start in the middle of the 10 mock nodes.
        object_list.down(&view);
        object_list.update(rect, &view);
        object_list.page_down(&view);
        object_list.update(rect, &view);

        object_list.last(&view);
        assert_eq!(object_list.selected, Some(ObjectId::from_raw_id(10)));

        object_list.first(&view);
        assert_eq!(object_list.selected, Some(ObjectId::from_raw_id(1)));
    }

    #[test]
    fn visible_objects_changes_with_scroll() {
        let (state, wirehose) = init();
        let view = View::from(
            &wirehose,
            &state,
            &config::Names::default(),
            &Vec::new(),
        );

        let height = NodeWidget::height() + NodeWidget::spacing();
        // 3 nodes + 2 lines for header and footer
        let rect = Rect::new(0, 0, 80, height * 3 + 2);
        let mut object_list =
            ObjectList::new(ListKind::Node(NodeKind::All), None);

        // Start at top
        let visible = object_list.visible_objects(&rect, &view);
        assert_eq!(visible.len(), 4);
        assert!(visible.contains(&ObjectId::from_raw_id(0)));
        assert!(visible.contains(&ObjectId::from_raw_id(1)));
        assert!(visible.contains(&ObjectId::from_raw_id(2)));
        assert!(visible.contains(&ObjectId::from_raw_id(3)));

        // Scroll down
        object_list.top = 5;
        let visible = object_list.visible_objects(&rect, &view);
        assert_eq!(visible.len(), 4);
        assert!(visible.contains(&ObjectId::from_raw_id(0)));
        assert!(visible.contains(&ObjectId::from_raw_id(6)));
        assert!(visible.contains(&ObjectId::from_raw_id(7)));
        assert!(visible.contains(&ObjectId::from_raw_id(8)));

        // Scroll up
        object_list.top = 4;
        let visible = object_list.visible_objects(&rect, &view);
        assert_eq!(visible.len(), 4);
        assert!(visible.contains(&ObjectId::from_raw_id(0)));
        assert!(visible.contains(&ObjectId::from_raw_id(5)));
        assert!(visible.contains(&ObjectId::from_raw_id(6)));
        assert!(visible.contains(&ObjectId::from_raw_id(7)));
    }

    #[test]
    fn visible_objects_includes_linked_clients() {
        let (mut state, wirehose) = init();

        // Set client_id on node 1
        let mut props = state
            .nodes
            .get(&ObjectId::from_raw_id(1))
            .unwrap()
            .props
            .clone();
        props.set_client_id(ObjectId::from_raw_id(101));
        state.update(StateEvent::NodeProperties {
            object_id: ObjectId::from_raw_id(1),
            props,
        });

        let view = View::from(
            &wirehose,
            &state,
            &config::Names::default(),
            &Vec::new(),
        );

        let height = NodeWidget::height() + NodeWidget::spacing();
        // 1 node + 2 lines for header and footer
        let rect = Rect::new(0, 0, 80, height + 2);
        let object_list = ObjectList::new(ListKind::Node(NodeKind::All), None);

        let visible = object_list.visible_objects(&rect, &view);
        assert_eq!(visible.len(), 3);
        assert!(visible.contains(&ObjectId::from_raw_id(0)));
        assert!(visible.contains(&ObjectId::from_raw_id(1)));
        assert!(visible.contains(&ObjectId::from_raw_id(101)));
    }

    #[test]
    fn visible_objects_includes_linked_devices() {
        let (mut state, wirehose) = init();

        // Set device_id on node 1
        let mut props = state
            .nodes
            .get(&ObjectId::from_raw_id(1))
            .unwrap()
            .props
            .clone();
        props.set_device_id(ObjectId::from_raw_id(101));
        let card_profile_device = 0;
        props.set_card_profile_device(card_profile_device);
        state.update(StateEvent::NodeProperties {
            object_id: ObjectId::from_raw_id(1),
            props,
        });

        // Create a test device with everything needed to populate device_info
        // on the node in the view.
        state.update(StateEvent::DeviceProperties {
            object_id: ObjectId::from_raw_id(101),
            props: PropertyStore::default(),
        });
        state.update(StateEvent::DeviceProfile {
            object_id: ObjectId::from_raw_id(101),
            index: 1,
        });
        state.update(StateEvent::DeviceRoute {
            object_id: ObjectId::from_raw_id(101),
            index: 0,
            device: card_profile_device,
            profiles: vec![1],
            description: String::new(),
            available: true,
            channel_volumes: vec![1.0],
            mute: false,
        });

        let view = View::from(
            &wirehose,
            &state,
            &config::Names::default(),
            &Vec::new(),
        );

        let height = NodeWidget::height() + NodeWidget::spacing();
        // 1 node + 2 lines for header and footer
        let rect = Rect::new(0, 0, 80, height + 2);
        let object_list = ObjectList::new(ListKind::Node(NodeKind::All), None);

        let visible = object_list.visible_objects(&rect, &view);
        assert_eq!(visible.len(), 3);
        assert!(visible.contains(&ObjectId::from_raw_id(0)));
        assert!(visible.contains(&ObjectId::from_raw_id(1)));
        assert!(visible.contains(&ObjectId::from_raw_id(101)));
    }

    #[test]
    fn visible_objects_includes_target() {
        let mut state = State::default();
        let wirehose = mock::WirehoseHandle::default();

        // Create a playback stream (sink input)
        let stream_id = ObjectId::from_raw_id(0);
        create_node(&mut state, stream_id, "Stream/Output/Audio", "stream");

        // Create a sink as the target
        let sink_id = ObjectId::from_raw_id(100);
        create_node(&mut state, sink_id, "Audio/Sink", "sink");

        // Create a link from stream to sink
        state.update(StateEvent::Link {
            object_id: ObjectId::from_raw_id(200),
            output_id: stream_id,
            input_id: sink_id,
        });

        // Set up metadata
        let metadata_id = ObjectId::from_raw_id(300);
        state.update(StateEvent::MetadataMetadataName {
            object_id: metadata_id,
            metadata_name: String::from("default"),
        });
        state.update(StateEvent::MetadataProperty {
            object_id: metadata_id,
            subject: u32::from(stream_id),
            key: Some(String::from("target.node")),
            value: Some(String::from("100")),
        });

        let view = View::from(
            &wirehose,
            &state,
            &config::Names::default(),
            &Vec::new(),
        );

        let height = NodeWidget::height() + NodeWidget::spacing();
        let rect = Rect::new(0, 0, 80, height + 2);
        let object_list =
            ObjectList::new(ListKind::Node(NodeKind::Playback), None);

        let visible = object_list.visible_objects(&rect, &view);
        assert!(visible.contains(&stream_id));
        assert!(visible.contains(&sink_id));
    }

    #[test]
    fn visible_objects_includes_target_client() {
        let mut state = State::default();
        let wirehose = mock::WirehoseHandle::default();

        // Create a playback stream
        let stream_id = ObjectId::from_raw_id(0);
        create_node(&mut state, stream_id, "Stream/Output/Audio", "stream");

        // Create a sink with a client_id
        let sink_id = ObjectId::from_raw_id(100);
        let sink_client_id = ObjectId::from_raw_id(101);
        let mut props = PropertyStore::default();
        props.set_node_description(String::from("Test sink"));
        props.set_media_class(String::from("Audio/Sink"));
        props.set_media_name(String::from("Media name"));
        props.set_node_name(String::from("sink"));
        props.set_object_serial(100);
        props.set_client_id(sink_client_id);
        state.update(StateEvent::NodeProperties {
            object_id: sink_id,
            props,
        });
        state.update(StateEvent::NodeVolumes {
            object_id: sink_id,
            volumes: vec![1.0, 1.0],
        });
        state.update(StateEvent::NodeMute {
            object_id: sink_id,
            mute: false,
        });

        // Create a link from stream to sink
        state.update(StateEvent::Link {
            object_id: ObjectId::from_raw_id(200),
            output_id: stream_id,
            input_id: sink_id,
        });

        // Set up metadata
        let metadata_id = ObjectId::from_raw_id(300);
        state.update(StateEvent::MetadataMetadataName {
            object_id: metadata_id,
            metadata_name: String::from("default"),
        });
        state.update(StateEvent::MetadataProperty {
            object_id: metadata_id,
            subject: u32::from(stream_id),
            key: Some(String::from("target.node")),
            value: Some(String::from("100")),
        });

        let view = View::from(
            &wirehose,
            &state,
            &config::Names::default(),
            &Vec::new(),
        );

        let height = NodeWidget::height() + NodeWidget::spacing();
        let rect = Rect::new(0, 0, 80, height + 2);
        let object_list =
            ObjectList::new(ListKind::Node(NodeKind::Playback), None);

        let visible = object_list.visible_objects(&rect, &view);
        assert!(visible.contains(&stream_id));
        assert!(visible.contains(&sink_id));
        assert!(visible.contains(&sink_client_id));
    }

    #[test]
    fn visible_objects_includes_target_device() {
        let mut state = State::default();
        let wirehose = mock::WirehoseHandle::default();

        // Create a playback stream
        let stream_id = ObjectId::from_raw_id(0);
        create_node(&mut state, stream_id, "Stream/Output/Audio", "stream");

        // Create a sink with device_info
        let sink_id = ObjectId::from_raw_id(100);
        let sink_device_id = ObjectId::from_raw_id(101);
        let card_profile_device = 0;
        let mut props = PropertyStore::default();
        props.set_node_description(String::from("Test sink"));
        props.set_media_class(String::from("Audio/Sink"));
        props.set_media_name(String::from("Media name"));
        props.set_node_name(String::from("sink"));
        props.set_object_serial(100);
        props.set_device_id(sink_device_id);
        props.set_card_profile_device(card_profile_device);
        state.update(StateEvent::NodeProperties {
            object_id: sink_id,
            props,
        });
        state.update(StateEvent::NodeVolumes {
            object_id: sink_id,
            volumes: vec![1.0, 1.0],
        });
        state.update(StateEvent::NodeMute {
            object_id: sink_id,
            mute: false,
        });

        // Create the device with route
        state.update(StateEvent::DeviceProperties {
            object_id: sink_device_id,
            props: PropertyStore::default(),
        });
        state.update(StateEvent::DeviceProfile {
            object_id: sink_device_id,
            index: 1,
        });
        state.update(StateEvent::DeviceRoute {
            object_id: sink_device_id,
            index: 0,
            device: card_profile_device,
            profiles: vec![1],
            description: String::new(),
            available: true,
            channel_volumes: vec![1.0],
            mute: false,
        });

        // Create a link from stream to sink
        state.update(StateEvent::Link {
            object_id: ObjectId::from_raw_id(200),
            output_id: stream_id,
            input_id: sink_id,
        });

        // Set up metadata
        let metadata_id = ObjectId::from_raw_id(300);
        state.update(StateEvent::MetadataMetadataName {
            object_id: metadata_id,
            metadata_name: String::from("default"),
        });
        state.update(StateEvent::MetadataProperty {
            object_id: metadata_id,
            subject: u32::from(stream_id),
            key: Some(String::from("target.node")),
            value: Some(String::from("100")),
        });

        let view = View::from(
            &wirehose,
            &state,
            &config::Names::default(),
            &Vec::new(),
        );

        let height = NodeWidget::height() + NodeWidget::spacing();
        let rect = Rect::new(0, 0, 80, height + 2);
        let object_list =
            ObjectList::new(ListKind::Node(NodeKind::Playback), None);

        let visible = object_list.visible_objects(&rect, &view);
        assert!(visible.contains(&stream_id));
        assert!(visible.contains(&sink_id));
        assert!(visible.contains(&sink_device_id));
    }

    #[test]
    fn visible_objects_includes_default_sink() {
        let mut state = State::default();
        let wirehose = mock::WirehoseHandle::default();

        // Create a playback stream (no explicit link - will use default)
        let stream_id = ObjectId::from_raw_id(0);
        create_node(&mut state, stream_id, "Stream/Output/Audio", "stream");

        // Create a sink
        let sink_id = ObjectId::from_raw_id(100);
        create_node(&mut state, sink_id, "Audio/Sink", "default_sink");

        // Set up metadata for the default sink
        let metadata_id = ObjectId::from_raw_id(300);
        state.update(StateEvent::MetadataMetadataName {
            object_id: metadata_id,
            metadata_name: String::from("default"),
        });
        state.update(StateEvent::MetadataProperty {
            object_id: metadata_id,
            subject: 0,
            key: Some(String::from("default.audio.sink")),
            value: Some(String::from("{\"name\":\"default_sink\"}")),
        });

        let view = View::from(
            &wirehose,
            &state,
            &config::Names::default(),
            &Vec::new(),
        );

        assert!(view.default_sink.is_some());

        let height = NodeWidget::height() + NodeWidget::spacing();
        let rect = Rect::new(0, 0, 80, height + 2);
        let object_list =
            ObjectList::new(ListKind::Node(NodeKind::Playback), None);

        let visible = object_list.visible_objects(&rect, &view);
        assert!(visible.contains(&stream_id));
        assert!(visible.contains(&sink_id));
    }

    #[test]
    fn visible_objects_includes_default_source() {
        let mut state = State::default();
        let wirehose = mock::WirehoseHandle::default();

        // Create a recording stream (no explicit link - will use default)
        let stream_id = ObjectId::from_raw_id(0);
        create_node(&mut state, stream_id, "Stream/Input/Audio", "stream");

        // Create a source
        let source_id = ObjectId::from_raw_id(100);
        create_node(&mut state, source_id, "Audio/Source", "default_source");

        // Set up metadata for the default source
        let metadata_id = ObjectId::from_raw_id(300);
        state.update(StateEvent::MetadataMetadataName {
            object_id: metadata_id,
            metadata_name: String::from("default"),
        });
        state.update(StateEvent::MetadataProperty {
            object_id: metadata_id,
            subject: 0,
            key: Some(String::from("default.audio.source")),
            value: Some(String::from("{\"name\":\"default_source\"}")),
        });

        let view = View::from(
            &wirehose,
            &state,
            &config::Names::default(),
            &Vec::new(),
        );

        assert!(view.default_source.is_some());

        let height = NodeWidget::height() + NodeWidget::spacing();
        let rect = Rect::new(0, 0, 80, height + 2);
        let object_list =
            ObjectList::new(ListKind::Node(NodeKind::Recording), None);

        let visible = object_list.visible_objects(&rect, &view);
        assert!(visible.contains(&stream_id));
        assert!(visible.contains(&source_id));
    }

    #[test]
    fn extend_selected_row_above_reaches_into_widened_clip() {
        let config = config::Config::from_toml_str(
            "row_selected_extend_above = true\n\
             [themes.default]\n\
             row_selected = { bg = \"Blue\" }",
        );
        let object_area = Rect::new(0, 1, 20, 3);
        // The row directly above object_area (y = 0) - out of bounds for
        // list_area (which starts at object_area's own top edge, as it
        // does for the first visible object), but in bounds once widened
        // to include header_area, matching what render_node_list()/
        // render_device_list() pass for the true first object in the list.
        let list_area = Rect::new(0, 1, 20, 10);
        let widened = Rect::new(0, 0, 20, 11);
        let blank = Buffer::empty(Rect::new(0, 0, 20, 11));

        let mut buf = blank.clone();
        extend_selected_row(
            &mut buf,
            &config,
            object_area,
            list_area,
            list_area,
            NodeWidget::spacing(),
        );
        assert_eq!(buf[(0, 0)].style(), blank[(0, 0)].style());

        let mut buf = blank.clone();
        extend_selected_row(
            &mut buf,
            &config,
            object_area,
            widened,
            list_area,
            NodeWidget::spacing(),
        );
        assert_ne!(buf[(0, 0)].style(), blank[(0, 0)].style());
    }

    #[test]
    fn extend_selected_row_below_reaches_into_widened_clip() {
        let config = config::Config::from_toml_str(
            "row_selected_extend_below = true\n\
             [themes.default]\n\
             row_selected = { bg = \"Blue\" }",
        );
        let object_area = Rect::new(0, 0, 20, 3);
        // The row directly below object_area (y = 3) - out of bounds for
        // a list_area that ends exactly at object_area's bottom edge, as
        // it does for the true last object in the list, but in bounds once
        // widened to include footer_area.
        let list_area = Rect::new(0, 0, 20, 3);
        let widened = Rect::new(0, 0, 20, 4);
        let blank = Buffer::empty(Rect::new(0, 0, 20, 4));

        let mut buf = blank.clone();
        extend_selected_row(
            &mut buf,
            &config,
            object_area,
            list_area,
            list_area,
            NodeWidget::spacing(),
        );
        assert_eq!(buf[(0, 3)].style(), blank[(0, 3)].style());

        let mut buf = blank.clone();
        extend_selected_row(
            &mut buf,
            &config,
            object_area,
            list_area,
            widened,
            NodeWidget::spacing(),
        );
        assert_ne!(buf[(0, 3)].style(), blank[(0, 3)].style());
    }

    #[test]
    fn render_divider_noop_when_disabled() {
        let config = config::Config::from_toml_str("show_dividers = false");
        let object_area = Rect::new(0, 0, 10, 3);
        let list_area = Rect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(list_area);

        render_divider(&mut buf, &config, object_area, list_area);

        let divider_row = Rect::new(0, 3, 10, 1);
        for cell in buf.content[buf.index_of(divider_row.x, divider_row.y)
            ..buf.index_of(divider_row.x, divider_row.y) + 10]
            .iter()
        {
            assert_eq!(cell.symbol(), " ");
        }
    }

    #[test]
    fn render_divider_draws_when_enabled() {
        let config = config::Config::from_toml_str("show_dividers = true");
        let object_area = Rect::new(0, 0, 10, 3);
        let list_area = Rect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(list_area);

        render_divider(&mut buf, &config, object_area, list_area);

        let divider_row_start = buf.index_of(0, 3);
        for cell in
            buf.content[divider_row_start..divider_row_start + 10].iter()
        {
            assert_eq!(cell.symbol(), config.char_set.divider);
        }
    }

    #[test]
    fn render_divider_clips_to_list_area() {
        let config = config::Config::from_toml_str("show_dividers = true");
        // object_area's bottom row falls just outside list_area's bottom
        // edge - nothing should be drawn, and this must not panic.
        let object_area = Rect::new(0, 8, 10, 3);
        let list_area = Rect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(list_area);

        render_divider(&mut buf, &config, object_area, list_area);
    }
}
