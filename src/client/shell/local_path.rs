//! Client-owned local path hit testing and transient hover presentation.
use super::*;
use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LocalPathHover {
    pane_id: String,
    path: String,
    area: Rect,
    source_area: crate::protocol::SurfaceRect,
    anchor: (u16, u16),
    boot_id: String,
    content_revision: u64,
    scroll: Option<crate::protocol::PaneSurfaceScrollMetrics>,
    cells: Vec<(u16, u16)>,
}

impl ClientShellState {
    fn local_path_interaction_enabled(&self) -> bool {
        self.active_endpoint_id.is_local()
            && self.mode == ClientShellMode::Terminal
            && self.overlay.is_none()
            && self.popup_terminal_id.is_none()
            && self.endpoint_error.is_none()
    }

    fn local_path_match_at_point(&self, column: u16, row: u16) -> Option<LocalPathHover> {
        if !self.local_path_interaction_enabled() || !self.local_path_surface_coherent() {
            return None;
        }
        let surface = self.pane_surface.as_ref()?;
        let snapshot = self.snapshot.as_ref()?;
        if snapshot.revision != surface.projection_revision {
            return None;
        }
        let hit = self
            .hits
            .panes
            .iter()
            .find(|hit| !hit.popup && hit.inner_rect.contains((column, row).into()))?;
        let pane = surface
            .panes
            .iter()
            .find(|pane| pane.pane_id == hit.pane_id)?;
        if pane.inner_rect.width != hit.inner_rect.width
            || pane.inner_rect.height != hit.inner_rect.height
        {
            return None;
        }
        let (path, cells) =
            path_in_surface(&surface.frame, pane.inner_rect, hit.inner_rect, column, row)?;
        Some(LocalPathHover {
            pane_id: hit.pane_id.clone(),
            path,
            area: hit.inner_rect,
            source_area: pane.inner_rect,
            anchor: (column, row),
            boot_id: surface.boot_id.clone(),
            content_revision: pane.content_revision,
            scroll: pane.scroll,
            cells,
        })
    }

    pub(super) fn local_path_at_point(&self, column: u16, row: u16) -> Option<(String, String)> {
        self.local_path_match_at_point(column, row)
            .map(|matched| (matched.pane_id, matched.path))
    }

    /// Returns whether a repaint is needed; unchanged paths reuse their cached span.
    pub(super) fn update_local_path_hover(&mut self, mouse: MouseEvent) -> bool {
        let anchor = (mouse.column, mouse.row);
        let next = if mouse.kind == MouseEventKind::Moved
            && mouse.modifiers.contains(KeyModifiers::CONTROL)
        {
            let retain = self.local_path_hover.as_ref().is_some_and(|hover| {
                (self.local_path_hover_is_current(hover) && hover.cells.contains(&anchor))
                    || (!self.local_path_surface_coherent()
                        && self.local_path_hover_target(hover).is_some()
                        && hover.area.contains(anchor.into()))
            });
            if retain {
                if let Some(hover) = self.local_path_hover.as_mut() {
                    hover.anchor = anchor;
                }
                return false;
            }
            self.local_path_match_at_point(mouse.column, mouse.row)
        } else {
            None
        };
        if self.local_path_hover == next {
            return false;
        }
        self.local_path_hover = next;
        true
    }

    fn local_path_surface_coherent(&self) -> bool {
        self.pending_pane_surface.is_none()
            && self
                .snapshot
                .as_ref()
                .zip(self.pane_surface.as_ref())
                .is_some_and(|(snapshot, surface)| {
                    snapshot.boot_id == surface.boot_id
                        && snapshot.revision == surface.projection_revision
                })
    }

    /// Content revisions can advance without invalidating the stationary pointer.
    /// A growing scrollback maximum is output growth, not a viewport scroll.
    fn local_path_hover_target(
        &self,
        hover: &LocalPathHover,
    ) -> Option<&crate::protocol::PaneSurfacePane> {
        if !self.local_path_interaction_enabled()
            || !self
                .hits
                .panes
                .iter()
                .any(|hit| hit.pane_id == hover.pane_id && hit.inner_rect == hover.area)
        {
            return None;
        }
        let surface = self.pane_surface.as_ref()?;
        if surface.boot_id != hover.boot_id {
            return None;
        }
        surface.panes.iter().find(|pane| {
            pane.pane_id == hover.pane_id
                && pane.inner_rect == hover.source_area
                && scroll_viewport(pane.scroll) == scroll_viewport(hover.scroll)
        })
    }

    fn local_path_hover_is_current(&self, hover: &LocalPathHover) -> bool {
        self.local_path_surface_coherent()
            && self
                .local_path_hover_target(hover)
                .is_some_and(|pane| pane.content_revision == hover.content_revision)
            && hover.cells.contains(&hover.anchor)
    }

    /// Refresh the one hovered row after output changes. A temporarily unmatched
    /// snapshot/surface pair suppresses rendering but retains the pointer intent.
    pub(super) fn clear_stale_local_path_hover(&mut self) {
        let Some(hover) = self.local_path_hover.as_ref() else {
            return;
        };
        if self.local_path_hover_target(hover).is_none() {
            self.local_path_hover = None;
            return;
        }
        if !self.local_path_surface_coherent() || self.local_path_hover_is_current(hover) {
            return;
        }
        let (column, row) = hover.anchor;
        self.local_path_hover = self.local_path_match_at_point(column, row);
    }

    pub(super) fn render_local_path_hover(&self, frame: &mut FrameData) {
        let Some(hover) = self
            .local_path_hover
            .as_ref()
            .filter(|hover| self.local_path_hover_is_current(hover))
        else {
            return;
        };
        for &(x, y) in &hover.cells {
            if x < frame.width && y < frame.height && hover.area.contains((x, y).into()) {
                let index = usize::from(y) * usize::from(frame.width) + usize::from(x);
                if let Some(cell) = frame.cells.get_mut(index) {
                    cell.modifier |= Modifier::UNDERLINED.bits();
                }
            }
        }
    }
}

fn scroll_viewport(
    scroll: Option<crate::protocol::PaneSurfaceScrollMetrics>,
) -> Option<(u64, u64)> {
    scroll.map(|scroll| (scroll.offset_from_bottom, scroll.viewport_rows))
}

fn frame_cell(frame: &FrameData, x: u16, y: u16) -> Option<&crate::protocol::CellData> {
    if x >= frame.width || y >= frame.height {
        return None;
    }
    frame
        .cells
        .get(usize::from(y) * usize::from(frame.width) + usize::from(x))
}

/// Read only the clicked physical row. OSC 8 targets retain full paths even
/// when their labels wrap. Plain text has no reliable soft-wrap metadata in the
/// stable surface protocol, so separate rows must not be joined speculatively.
fn path_in_surface(
    frame: &FrameData,
    source: crate::protocol::SurfaceRect,
    screen: Rect,
    column: u16,
    row: u16,
) -> Option<(String, Vec<(u16, u16)>)> {
    if !screen.contains((column, row).into())
        || screen.width != source.width
        || screen.height != source.height
    {
        return None;
    }
    let local_row = row - screen.y;
    let clicked_col = column - screen.x;
    let mut line = String::new();
    let mut positions = Vec::new();
    let mut col = 0;
    let mut clicked_link = None;
    while col < source.width {
        let cell = frame_cell(
            frame,
            source.x.checked_add(col)?,
            source.y.checked_add(local_row)?,
        )?;
        let width = u16::try_from(cell.symbol.width().max(1))
            .ok()?
            .min(source.width - col);
        if col <= clicked_col && clicked_col < col + width {
            clicked_link = cell.hyperlink;
        }
        let start = line.len();
        if cell.symbol.is_empty() {
            line.push(' ');
        } else {
            line.push_str(&cell.symbol);
        }
        positions.push((start..line.len(), col, width));
        col += width;
    }
    if let Some(link) = clicked_link {
        let uri = frame.hyperlinks.get(usize::try_from(link).ok()?)?;
        let path = crate::local_path::path_from_file_uri(uri)?;
        let mut cells = Vec::new();
        for y in 0..source.height {
            let mut x = 0;
            while x < source.width {
                let cell = frame_cell(frame, source.x.checked_add(x)?, source.y.checked_add(y)?)?;
                let width = u16::try_from(cell.symbol.width().max(1))
                    .ok()?
                    .min(source.width - x);
                if cell.hyperlink == Some(link) {
                    for offset in 0..width {
                        cells.push((screen.x + x + offset, screen.y + y));
                    }
                }
                x += width;
            }
        }
        return Some((path, cells));
    }
    let matched = crate::local_path::path_match_at_column(&line, clicked_col)?;
    let mut cells = Vec::new();
    for (bytes, x, width) in positions {
        if bytes.start < matched.byte_range.end && matched.byte_range.start < bytes.end {
            for offset in 0..width {
                cells.push((screen.x + x + offset, row));
            }
        }
    }
    Some((matched.path, cells))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_frame(text: &str) -> FrameData {
        FrameData::from_ratatui_buffer_with_hyperlinks(&Buffer::with_lines([text]), None, &[])
    }

    fn shell_with_path() -> ClientShellState {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.overlay = None;
        state.snapshot = Some(Box::new(super::super::tests::snapshot()));
        let frame = text_frame("文件：/tmp/中文.txt,");
        let area = Rect::new(0, 0, frame.width, 1);
        state.pane_surface = Some(PaneSurfaceFrame {
            boot_id: "boot-1".into(),
            projection_revision: 1,
            surface_revision: 1,
            frame,
            panes: vec![crate::protocol::PaneSurfacePane {
                pane_id: "pane_1".into(),
                content_revision: 4,
                rect: area.into(),
                inner_rect: area.into(),
                scrollbar_rect: None,
                scroll: None,
                focused: true,
                mouse_reporting: false,
                sgr_pixel_mouse: false,
                alternate_screen_active: false,
                pixel_width: 0,
                pixel_height: 0,
            }],
            splits: Vec::new(),
            popup: None,
            graphics: crate::protocol::SurfaceGraphicsScene::default(),
        });
        state.hits.panes = vec![PaneHit {
            rect: area,
            inner_rect: area,
            scrollbar_rect: None,
            scroll: None,
            pane_id: "pane_1".into(),
            popup: false,
            mouse_reporting: false,
            sgr_pixel_mouse: false,
            pixel_width: 0,
            pixel_height: 0,
        }];
        state
    }

    fn ctrl_motion() -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Moved,
            column: 8,
            row: 0,
            modifiers: KeyModifiers::CONTROL,
        }
    }

    #[test]
    fn hover_updates_only_on_ctrl_motion_and_draws_only_path_without_changing_other_style() {
        let mut state = shell_with_path();
        assert!(state.update_local_path_hover(ctrl_motion()));
        assert!(!state.update_local_path_hover(ctrl_motion()));
        assert_eq!(
            state.local_path_at_point(8, 0),
            Some(("pane_1".into(), "/tmp/中文.txt".into()))
        );
        let mut frame = state.pane_surface.as_ref().unwrap().frame.clone();
        for cell in &mut frame.cells {
            cell.modifier = (Modifier::BOLD | Modifier::ITALIC).bits();
        }
        let before = frame.clone();
        state.render_local_path_hover(&mut frame);
        for (index, cell) in frame.cells.iter().enumerate() {
            let mut expected = before.cells[index].clone();
            if (6..frame.width - 1).contains(&(index as u16)) {
                expected.modifier |= Modifier::UNDERLINED.bits();
            }
            assert_eq!(*cell, expected);
        }
        let mut mouse = ctrl_motion();
        mouse.modifiers = KeyModifiers::NONE;
        assert!(state.update_local_path_hover(mouse));
        assert!(state.local_path_hover.is_none());
    }

    fn replace_surface_text(state: &mut ClientShellState, text: &str) {
        let surface = state.pane_surface.as_mut().unwrap();
        let mut buffer = Buffer::empty(Rect::new(0, 0, surface.frame.width, surface.frame.height));
        buffer.set_string(0, 0, text, Style::default());
        surface.frame = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
        surface.panes[0].content_revision += 2;
    }

    #[test]
    fn stationary_hover_refreshes_through_output_and_scrollback_growth() {
        let mut state = shell_with_path();
        let metrics = crate::protocol::PaneSurfaceScrollMetrics {
            offset_from_bottom: 0,
            max_offset_from_bottom: 10,
            viewport_rows: 1,
        };
        state.pane_surface.as_mut().unwrap().panes[0].scroll = Some(metrics);
        state.update_local_path_hover(ctrl_motion());
        let cells = state.local_path_hover.as_ref().unwrap().cells.clone();
        for _ in 0..3 {
            let surface = state.pane_surface.as_mut().unwrap();
            surface.panes[0].content_revision += 2;
            surface.panes[0]
                .scroll
                .as_mut()
                .unwrap()
                .max_offset_from_bottom += 1;
            state.clear_stale_local_path_hover();
            assert_eq!(state.local_path_hover.as_ref().unwrap().cells, cells);
            let mut frame = state.pane_surface.as_ref().unwrap().frame.clone();
            state.render_local_path_hover(&mut frame);
            for &(x, y) in &cells {
                assert_ne!(
                    frame_cell(&frame, x, y).unwrap().modifier & Modifier::UNDERLINED.bits(),
                    0
                );
            }
        }
    }

    #[test]
    fn changed_or_disappearing_path_does_not_reuse_the_old_underline_range() {
        let mut state = shell_with_path();
        state.update_local_path_hover(ctrl_motion());
        replace_surface_text(&mut state, "文件：/a/b,");
        let mut frame = state.pane_surface.as_ref().unwrap().frame.clone();
        let before = frame.clone();
        state.render_local_path_hover(&mut frame);
        assert_eq!(frame, before, "old span must not render before refresh");
        state.clear_stale_local_path_hover();
        let hover = state.local_path_hover.as_ref().unwrap();
        assert_eq!(hover.path, "/a/b");
        assert_eq!(hover.cells, vec![(6, 0), (7, 0), (8, 0), (9, 0)]);
        replace_surface_text(&mut state, "the path disappeared");
        state.clear_stale_local_path_hover();
        assert!(state.local_path_hover.is_none());
    }

    #[test]
    fn transient_surface_mismatch_retains_pointer_intent_and_recovers_without_motion() {
        for pending in [false, true] {
            let mut state = shell_with_path();
            state.update_local_path_hover(ctrl_motion());
            if pending {
                state.pending_pane_surface = state.pane_surface.clone();
            } else {
                state.snapshot.as_mut().unwrap().revision += 1;
            }
            state.clear_stale_local_path_hover();
            assert!(state.local_path_hover.is_some());
            let mut mouse = ctrl_motion();
            mouse.column += 1;
            state.update_local_path_hover(mouse);
            assert_eq!(state.local_path_hover.as_ref().unwrap().anchor, (9, 0));
            let mut frame = state.pane_surface.as_ref().unwrap().frame.clone();
            let before = frame.clone();
            state.render_local_path_hover(&mut frame);
            assert_eq!(
                frame, before,
                "unmatched surfaces must not draw cached spans"
            );
            state.pending_pane_surface = None;
            state.pane_surface.as_mut().unwrap().projection_revision =
                state.snapshot.as_ref().unwrap().revision;
            state.pane_surface.as_mut().unwrap().panes[0].content_revision += 2;
            state.clear_stale_local_path_hover();
            assert!(state
                .local_path_hover
                .as_ref()
                .is_some_and(|hover| state.local_path_hover_is_current(hover)));
        }
    }

    #[test]
    fn pointer_moves_during_mismatch_refresh_the_latest_anchor_or_clear_outside_the_pane() {
        let mut state = shell_with_path();
        state.update_local_path_hover(ctrl_motion());
        state.snapshot.as_mut().unwrap().revision += 1;
        let mut mouse = ctrl_motion();
        mouse.column = 1; // Still in the pane, but now over prose rather than the path.
        state.update_local_path_hover(mouse);
        assert!(state.local_path_hover.is_some());
        state.pane_surface.as_mut().unwrap().projection_revision += 1;
        state.clear_stale_local_path_hover();
        assert!(state.local_path_hover.is_none());
        state.update_local_path_hover(ctrl_motion());
        state.snapshot.as_mut().unwrap().revision += 1;
        mouse.column = 100;
        state.update_local_path_hover(mouse);
        assert!(state.local_path_hover.is_none());
    }

    #[test]
    fn host_replies_and_control_repeats_retain_hover_but_user_input_and_focus_loss_clear_it() {
        use crate::raw_input::RawInputEvent;
        use crossterm::event::{KeyEventKind, ModifierKeyCode};
        let control = crate::input::TerminalKey::new(
            KeyCode::Modifier(ModifierKeyCode::LeftControl),
            KeyModifiers::CONTROL,
        );
        let mut state = shell_with_path();
        state.update_local_path_hover(ctrl_motion());
        let color = crate::terminal_theme::RgbColor {
            r: 10,
            g: 20,
            b: 30,
        };
        for event in [
            RawInputEvent::HostDefaultColor {
                kind: crate::terminal_theme::DefaultColorKind::Foreground,
                color,
            },
            RawInputEvent::HostPaletteColors {
                colors: vec![(1, color)],
            },
            RawInputEvent::HostCellSizeReport {
                width_px: 8,
                height_px: 16,
            },
            RawInputEvent::Unsupported,
            RawInputEvent::Key(control.clone()),
            RawInputEvent::Key(control.clone().with_kind(KeyEventKind::Repeat)),
        ] {
            state.handle_raw_events(vec![event]);
            assert!(state.local_path_hover.is_some());
        }
        for event in [
            RawInputEvent::Key(control.with_kind(KeyEventKind::Release)),
            RawInputEvent::Key(crate::input::TerminalKey::new(
                KeyCode::Char('x'),
                KeyModifiers::NONE,
            )),
            RawInputEvent::Paste("hello".into()),
            RawInputEvent::OuterFocusLost,
        ] {
            state.update_local_path_hover(ctrl_motion());
            assert!(state.local_path_hover.is_some());
            state.handle_raw_events(vec![event]);
            assert!(state.local_path_hover.is_none());
        }
    }

    #[test]
    fn hover_invalidates_on_scroll_geometry_and_mode_changes() {
        for case in 0..5 {
            let mut state = shell_with_path();
            assert!(state.update_local_path_hover(ctrl_motion()));
            match case {
                0 => {
                    state.pane_surface.as_mut().unwrap().panes[0].scroll =
                        Some(crate::protocol::PaneSurfaceScrollMetrics {
                            offset_from_bottom: 1,
                            max_offset_from_bottom: 2,
                            viewport_rows: 1,
                        })
                }
                1 => state.hits.panes[0].inner_rect.x += 1,
                2 => state.pane_surface.as_mut().unwrap().panes[0].inner_rect.x += 1,
                3 => state.pane_surface.as_mut().unwrap().boot_id = "new-boot".into(),
                _ => state.mode = ClientShellMode::Copy,
            }
            let mut frame = state.pane_surface.as_ref().unwrap().frame.clone();
            let before = frame.clone();
            state.render_local_path_hover(&mut frame);
            assert_eq!(frame, before);
            state.clear_stale_local_path_hover();
            assert!(state.local_path_hover.is_none());
        }
    }

    #[test]
    #[ignore = "manual client hover scaling profile; reports timings without a wall-clock threshold"]
    fn local_path_hover_render_scale_profile() {
        use std::hint::black_box;
        use std::time::{Duration, Instant};
        const ITERATIONS: u32 = 100_000;
        for pane_count in [1, 15] {
            let mut state = shell_with_path();
            let pane = state.pane_surface.as_ref().unwrap().panes[0].clone();
            let hit = state.hits.panes[0].clone();
            // Put the hovered pane last to measure the longest matching lookup.
            for index in 1..pane_count {
                let mut extra_pane = pane.clone();
                extra_pane.pane_id = format!("extra_{index}");
                extra_pane.rect.y = index * 2;
                extra_pane.inner_rect.y = index * 2;
                state
                    .pane_surface
                    .as_mut()
                    .unwrap()
                    .panes
                    .insert(0, extra_pane);
                let mut extra_hit = hit.clone();
                extra_hit.pane_id = format!("extra_{index}");
                extra_hit.rect.y = index * 2;
                extra_hit.inner_rect.y = index * 2;
                state.hits.panes.insert(0, extra_hit);
            }
            state.local_path_hover = state.local_path_match_at_point(8, 0);
            let hover = state.local_path_hover.take().unwrap();
            let buffer = Buffer::empty(Rect::new(0, 0, 120, 40));
            let mut frame = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
            let mut medians = [Duration::ZERO; 3];
            for (case, median) in medians.iter_mut().enumerate() {
                state.local_path_hover = (case != 0).then(|| hover.clone());
                state
                    .pane_surface
                    .as_mut()
                    .unwrap()
                    .panes
                    .last_mut()
                    .unwrap()
                    .content_revision = hover.content_revision;
                let mut samples = [Duration::ZERO; 3];
                for elapsed in &mut samples {
                    let start = Instant::now();
                    for _ in 0..ITERATIONS {
                        if case == 2 {
                            state
                                .pane_surface
                                .as_mut()
                                .unwrap()
                                .panes
                                .last_mut()
                                .unwrap()
                                .content_revision += 2;
                        }
                        black_box(&mut state).clear_stale_local_path_hover();
                        black_box(&state).render_local_path_hover(black_box(&mut frame));
                    }
                    *elapsed = start.elapsed();
                }
                samples.sort();
                *median = samples[1];
            }
            let none = medians[0].as_nanos() as f64 / f64::from(ITERATIONS);
            let unchanged = medians[1].as_nanos() as f64 / f64::from(ITERATIONS);
            let changed = medians[2].as_nanos() as f64 / f64::from(ITERATIONS);
            eprintln!("client local_path_hover_render_scale_profile: panes={pane_count}, geometry=120x40, refresh+render none={none:.1} ns/frame, unchanged={unchanged:.1} ns/frame, changed_parse={changed:.1} ns/frame");
        }
    }

    #[test]
    fn surface_path_maps_chinese_text_and_wide_cells_to_screen_coordinates() {
        let frame = text_frame("• 文件：/home/中文/README.md,");
        let source = Rect::new(0, 0, frame.width, frame.height);
        let screen = Rect::new(5, 7, frame.width, frame.height);
        let (path, cells) = path_in_surface(&frame, source.into(), screen, 20, 7).unwrap();
        assert_eq!(path, "/home/中文/README.md");
        let first = 5 + "• 文件：".width() as u16;
        assert_eq!(cells.first(), Some(&(first, 7)));
        assert_eq!(cells.last(), Some(&(5 + frame.width - 2, 7)));
        assert!(path_in_surface(&frame, source.into(), screen, 8, 7).is_none());
        assert!(path_in_surface(&frame, source.into(), screen, 5 + frame.width - 1, 7).is_none());
        let chinese = first + "/home/".width() as u16;
        let (second_half, _) =
            path_in_surface(&frame, source.into(), screen, chinese + 1, 7).unwrap();
        assert_eq!(second_half, path);
    }

    #[test]
    fn file_hyperlinks_resolve_short_labels_and_preserve_web_link_behavior() {
        let mut frame = text_frame("说明");
        frame.hyperlinks.push("file:///tmp/my%20file.md".into());
        frame.cells[0].hyperlink = Some(0);
        frame.cells[2].hyperlink = Some(0);
        let area = Rect::new(0, 0, frame.width, 1);
        let (path, cells) = path_in_surface(&frame, area.into(), area, 1, 0).unwrap();
        assert_eq!(path, "/tmp/my file.md");
        assert_eq!(cells, vec![(0, 0), (1, 0), (2, 0), (3, 0)]);
        frame.hyperlinks[0] = "https://example.com/a".into();
        assert!(path_in_surface(&frame, area.into(), area, 1, 0).is_none());
    }

    #[test]
    fn surface_paths_do_not_join_unrelated_rows_or_accept_truncated_geometry() {
        let frame = text_frame("`./my project/file.rs:12` next");
        let area = Rect::new(0, 0, frame.width, 1);
        let (path, cells) = path_in_surface(&frame, area.into(), area, 8, 0).unwrap();
        assert_eq!(path, "./my project/file.rs");
        assert_eq!(cells.first(), Some(&(1, 0)));
        assert_eq!(
            cells.last(),
            Some(&("./my project/file.rs:12".width() as u16, 0))
        );
        assert!(path_in_surface(&frame, area.into(), Rect::new(0, 0, 100, 1), 8, 0).is_none());
    }
}
