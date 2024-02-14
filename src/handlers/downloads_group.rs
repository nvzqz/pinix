use std::collections::HashMap;

use console::style;
use indexmap::IndexMap;
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};
use once_cell::sync::Lazy;

use crate::action::{Action, ActionType, BuildStepId, ResultFields, StartFields};
use crate::state::{Handler, HandlerResult, State};
use crate::style::{format_short_build_target, template_style, MultiBar};

static C_RUN: Lazy<String> = Lazy::new(|| style("-").blue().bright().to_string());

fn get_style(size: u16) -> ProgressStyle {
    template_style(
        size,
        true,
        |size| match size {
            0..=50 => "{wide_msg}",
            51..=60 => "{wide_msg} {binary_bytes_per_sec:^12}",
            _ => "{wide_msg} {binary_bytes_per_sec:^12} {bytes:^12}",
        },
        |_| "[{prefix}]",
    )
}

pub fn handle_new_downloads_group(state: &mut State, action: &Action) -> HandlerResult {
    if let Action::Start {
        action_type: ActionType::CopyPaths,
        id,
        ..
    } = action
    {
        let handler = DownloadsGroup::new(*id, state);
        state.plug(handler);
    }

    HandlerResult::Continue
}

struct DownloadsGroup {
    id: BuildStepId,
    progress: ProgressBar,
    current_copies: IndexMap<BuildStepId, String>,
    state_copy: HashMap<BuildStepId, [u64; 4]>,
    state_transfer: HashMap<BuildStepId, [u64; 4]>,
    state_self: [u64; 4],
    max_copy: u64,
    max_transfer: u64,
}

impl DownloadsGroup {
    fn new(id: BuildStepId, state: &mut State) -> Self {
        let progress = ProgressBar::new_spinner().with_style(get_style(state.term_size));

        Self {
            id,
            progress: state.add(progress),
            current_copies: IndexMap::new(),
            state_copy: HashMap::new(),
            state_transfer: HashMap::new(),
            state_self: [0; 4],
            max_copy: 0,
            max_transfer: 0,
        }
    }

    fn get_done(&self) -> u64 {
        self.state_transfer.values().map(|&[done, ..]| done).sum()
    }

    fn get_running(&self) -> u64 {
        self.state_transfer
            .values()
            .map(|&[_, expected, ..]| expected)
            .sum()
    }

    fn get_unpacked(&self) -> u64 {
        self.state_copy.values().map(|&[done, ..]| done).sum()
    }

    fn update_bar(&self, term_size: u16) {
        let size = u64::from(term_size / 3);
        let done = self.get_done();
        let expected = self.max_transfer;
        let running = self.get_running();

        let adv1 = (size * done + expected / 2)
            .checked_div(expected)
            .unwrap_or(0);

        let adv2 = (size * running + expected / 2)
            .checked_div(expected)
            .unwrap_or(0);

        let c_pos = "#";

        self.progress.set_prefix(
            MultiBar([
                (c_pos, adv1),
                (C_RUN.as_str(), adv2 - adv1),
                (" ", size - adv2),
            ])
            .to_string(),
        );
    }

    fn update_message(&self) {
        let pkgs = self
            .current_copies
            .values()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");

        self.progress.set_message(format!(
            "Downloaded ({}/{}) {pkgs}",
            self.state_self[0], self.state_self[1],
        ));
    }
}

impl Handler for DownloadsGroup {
    fn handle(&mut self, state: &mut State, action: &Action) -> HandlerResult {
        match action {
            Action::Start {
                action_type: ActionType::CopyPath,
                id,
                fields: StartFields::Copy([path, _, _]),
                ..
            } => {
                self.state_copy.insert(*id, [0; 4]);
                self.current_copies
                    .insert(*id, format_short_build_target(path));
            }

            Action::Start {
                action_type: ActionType::FileTransfer,
                id,
                ..
            } => {
                self.state_transfer.insert(*id, [0; 4]);
            }

            Action::Result {
                action_type: ActionType::Build,
                id,
                fields: ResultFields::Progress(dl_state),
            } => {
                if *id == self.id {
                    self.state_self = *dl_state;
                    self.update_message();
                }

                if let Some(copy) = self.state_copy.get_mut(id) {
                    *copy = *dl_state;
                }

                if let Some(transfer) = self.state_transfer.get_mut(id) {
                    *transfer = *dl_state;
                    self.progress.set_position(self.get_done());
                    self.update_bar(state.term_size);
                }
            }

            Action::Result {
                action_type: ActionType::OptimiseStore,
                fields: ResultFields::Realise(ActionType::CopyPath, max_copy),
                ..
            } => {
                self.max_copy = *max_copy;
            }

            Action::Result {
                action_type: ActionType::OptimiseStore,
                fields: ResultFields::Realise(ActionType::FileTransfer, max_transfer),
                ..
            } => {
                self.max_transfer = *max_transfer;
                self.progress.set_length(self.max_transfer);
                self.update_bar(state.term_size);
            }

            Action::Stop { id } if *id == self.id => {
                if self.state_self[0] > 0 {
                    let msg_main = format!(
                        "{} Downloaded {} derivations",
                        style("⬇").green(),
                        self.state_self[0],
                    );

                    let msg_stats = style(format!(
                        " ({} downloaded / {} unpacked, {:.0?})",
                        HumanBytes(self.get_done()),
                        HumanBytes(self.get_unpacked()),
                        self.progress.duration(),
                    ))
                    .dim()
                    .to_string();

                    state.println(msg_main + &msg_stats);
                    self.progress.finish_and_clear();
                }

                self.progress.finish_and_clear();
                return HandlerResult::Close;
            }

            Action::Stop { id } => {
                self.current_copies.shift_remove(id);
                self.update_message();
            }

            _ => {}
        }

        HandlerResult::Continue
    }

    fn resize(&mut self, _state: &mut State, size: u16) {
        self.progress.set_style(get_style(size));
        self.update_bar(size);
    }
}
