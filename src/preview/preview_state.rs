use {
    super::*,
    crate::{
        app::*,
        command::{Command, ScrollCommand, TriggerType},
        display::{CropWriter, LONG_SPACE, Screen, W},
        errors::ProgramError,
        flag::Flag,
        pattern::InputPattern,
        skin::PanelSkin,
        task_sync::Dam,
        verb::*,
    },
    crossterm::{
        cursor,
        QueueableCommand,
    },
    std::path::{Path, PathBuf},
    termimad::Area,
};

/// an application state dedicated to previewing files.
/// It's usually the only state in its panel and is kept when
/// the selection changes (other panels indirectly call
/// set_selected_path).
pub struct PreviewState {
    pub preview_area: Area,
    dirty: bool, // true when background must be cleared
    path: PathBuf, // path to the previewed file
    preview: Preview,
    pending_pattern: InputPattern, // a pattern (or not) which has not yet be applied
    filtered_preview: Option<Preview>,
    removed_pattern: InputPattern,
    prefered_mode: Option<PreviewMode>,
}

impl PreviewState {
    pub fn new(
        path: PathBuf,
        pending_pattern: InputPattern,
        prefered_mode: Option<PreviewMode>,
        con: &AppContext,
    ) -> PreviewState {
        let preview_area = Area::uninitialized(); // will be fixed at drawing time
        let preview = Preview::new(&path, prefered_mode, con);
        PreviewState {
            preview_area,
            dirty: true,
            path,
            preview,
            pending_pattern,
            filtered_preview: None,
            removed_pattern: InputPattern::none(),
            prefered_mode,
        }
    }
    fn mut_preview(&mut self) -> &mut Preview {
        self.filtered_preview.as_mut().unwrap_or(&mut self.preview)
    }
    fn set_mode(
        &mut self,
        mode: PreviewMode,
        con: &AppContext,
    ) -> Result<AppStateCmdResult, ProgramError> {
        if self.preview.get_mode() == Some(mode) {
            return Ok(AppStateCmdResult::Keep);
        }
        Ok(match Preview::with_mode(&self.path, mode, con) {
            Ok(preview) => {
                self.preview = preview;
                self.prefered_mode = Some(mode);
                AppStateCmdResult::Keep
            }
            Err(e) => {
                AppStateCmdResult::DisplayError(
                    format!("Can't display as {:?} : {:?}", mode, e)
                )
            }
        })
    }
}

impl AppState for PreviewState {

    fn get_pending_task(&self) -> Option<&'static str> {
        if self.pending_pattern.is_some() {
            Some("searching")
        } else {
            None
        }
    }

    fn on_pattern(
        &mut self,
        pat: InputPattern,
        _con: &AppContext,
    ) -> Result<AppStateCmdResult, ProgramError> {
        if pat.is_none() {
            if let Some(filtered_preview) = self.filtered_preview.take() {
                let old_selection = filtered_preview.get_selected_line_number();
                if let Some(number) = old_selection {
                    self.preview.try_select_line_number(number);
                }
                self.removed_pattern = filtered_preview.pattern();
            }
        } else {
            if !self.preview.is_filterable() {
                return Ok(AppStateCmdResult::DisplayError(
                    "this preview can't be searched".to_string()
                ));
            }
        }
        self.pending_pattern = pat;
        Ok(AppStateCmdResult::Keep)
    }

    /// do the preview filtering if required and not yet done
    fn do_pending_task(
        &mut self,
        _screen: &mut Screen,
        con: &AppContext,
        dam: &mut Dam,
    ) {
        if self.pending_pattern.is_some() {
            let old_selection = self
                .filtered_preview.as_ref().and_then(|p| p.get_selected_line_number())
                .or_else(|| self.preview.get_selected_line_number());
            let pattern = self.pending_pattern.take();
            self.filtered_preview = time!(
                Info,
                "preview filtering",
                self.preview.filtered(&self.path, pattern, dam, con),
            ); // can be None if a cancellation was required
            if let Some(ref mut filtered_preview) = self.filtered_preview {
                if let Some(number) = old_selection {
                    filtered_preview.try_select_line_number(number);
                }
            }
        }
    }

    fn selected_path(&self) -> &Path {
        &self.path
    }

    fn set_selected_path(&mut self, path: PathBuf, con: &AppContext) {
        if let Some(fp) = &self.filtered_preview {
            self.pending_pattern = fp.pattern();
        };
        self.preview = Preview::new(&path, self.prefered_mode, con);
        self.path = path;
    }

    fn selection(&self) -> Selection<'_> {
        Selection {
            path: &self.path,
            stype: SelectionType::File,
            line: self.preview.get_selected_line_number().unwrap_or(0),
        }
    }

    fn refresh(&mut self, _screen: &Screen, con: &AppContext) -> Command {
        self.dirty = true;
        self.set_selected_path(self.path.clone(), con);
        Command::empty()
    }

    fn on_click(
        &mut self,
        _x: u16,
        y: u16,
        _screen: &mut Screen,
        _con: &AppContext,
    ) -> Result<AppStateCmdResult, ProgramError> {
        if y >= self.preview_area.top  && y < self.preview_area.top + self.preview_area.height {
            let y = y - self.preview_area.top;
            self.mut_preview().try_select_y(y);
        }
        Ok(AppStateCmdResult::Keep)
    }

    fn display(
        &mut self,
        w: &mut W,
        screen: &Screen,
        state_area: Area,
        panel_skin: &PanelSkin,
        con: &AppContext,
    ) -> Result<(), ProgramError> {
        if state_area.height < 3 {
            warn!("area too small for preview");
            return Ok(());
        }
        let mut preview_area = state_area.clone();
        preview_area.height -= 1;
        preview_area.top += 1;
        if preview_area != self.preview_area {
            self.dirty = true;
            self.preview_area = preview_area;
        }
        if self.dirty {
            panel_skin.styles.default.queue_bg(w)?;
            screen.clear_area_to_right(w, &state_area)?;
            self.dirty = false;
        }
        let styles = &panel_skin.styles;
        w.queue(cursor::MoveTo(state_area.left, 0))?;
        let mut cw = CropWriter::new(w, state_area.width as usize);
        let file_name = self.path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "???".to_string());
        cw.queue_str(&styles.default, &file_name)?;
        let info_area = Area::new(
            state_area.left + state_area.width - cw.allowed as u16,
            state_area.top,
            cw.allowed as u16,
            1,
        );
        cw.fill(&styles.default, LONG_SPACE)?;
        let preview = self.filtered_preview.as_mut().unwrap_or(&mut self.preview);
        preview.display_info(w, screen, panel_skin, &info_area)?;
        preview.display(w, screen, panel_skin, &self.preview_area, con)
    }

    fn no_verb_status(
        &self,
        has_previous_state: bool,
        con: &AppContext,
    ) -> Status {
        let mut ssb = con.standard_status.builder(
            AppStateType::Preview,
            self.selection(),
        );
        ssb.has_previous_state = has_previous_state;
        ssb.is_filtered = self.filtered_preview.is_some();
        ssb.has_removed_pattern = self.removed_pattern.is_some();
        ssb.status()
    }

    fn on_internal(
        &mut self,
        w: &mut W,
        internal_exec: &InternalExecution,
        input_invocation: Option<&VerbInvocation>,
        trigger_type: TriggerType,
        cc: &CmdContext,
        screen: &mut Screen,
    ) -> Result<AppStateCmdResult, ProgramError> {
        match internal_exec.internal {
            Internal::back => {
                if self.filtered_preview.is_some() {
                    self.on_pattern(InputPattern::none(), &cc.con)
                } else {
                    Ok(AppStateCmdResult::PopState)
                }
            }
            Internal::line_down => {
                self.mut_preview().select_next_line();
                Ok(AppStateCmdResult::Keep)
            }
            Internal::line_up => {
                self.mut_preview().select_previous_line();
                Ok(AppStateCmdResult::Keep)
            }
            Internal::page_down => {
                self.mut_preview().try_scroll(ScrollCommand::Pages(1));
                Ok(AppStateCmdResult::Keep)
            }
            Internal::page_up => {
                self.mut_preview().try_scroll(ScrollCommand::Pages(-1));
                Ok(AppStateCmdResult::Keep)
            }
            //Internal::restore_pattern => {
            //    debug!("restore_pattern");
            //    self.pending_pattern = self.removed_pattern.take();
            //    Ok(AppStateCmdResult::Keep)
            //}
            Internal::panel_left if self.removed_pattern.is_some() => {
                debug!("restoring pattern");
                self.pending_pattern = self.removed_pattern.take();
                Ok(AppStateCmdResult::Keep)
            }
            Internal::panel_right if self.filtered_preview.is_some() => {
                self.on_pattern(InputPattern::none(), &cc.con)
            }
            Internal::select_first => {
                self.mut_preview().select_first();
                Ok(AppStateCmdResult::Keep)
            }
            Internal::select_last => {
                self.mut_preview().select_last();
                Ok(AppStateCmdResult::Keep)
            }
            Internal::preview_image => self.set_mode(PreviewMode::Image, cc.con),
            Internal::preview_text => self.set_mode(PreviewMode::Text, cc.con),
            Internal::preview_binary => self.set_mode(PreviewMode::Hex, cc.con),
            _ => self.on_internal_generic(
                w,
                internal_exec,
                input_invocation,
                trigger_type,
                cc,
                screen,
            ),
        }
    }

    fn get_flags(&self) -> Vec<Flag> {
        vec![]
    }

    fn get_starting_input(&self) -> String {
        if let Some(preview) = &self.filtered_preview {
            preview.pattern().raw
        } else {
            self.pending_pattern.raw.clone()
        }
    }

}
