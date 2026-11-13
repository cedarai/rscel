//! Optional bounded observation of actual default-clock operands.
//! Legacy evaluation always returns the same clock it read. Replay is confined
//! to a native comparison and never replaces the authoritative Python clock.
use crate::CelError;
use chrono::{DateTime, Utc};
use std::{cell::RefCell, rc::Rc};
const CAP: usize = 8192;
enum Mode {
    Observe {
        values: Vec<(i64, u32)>,
        complete: bool,
    },
    Replay {
        values: Vec<(i64, u32)>,
        index: usize,
        complete: bool,
    },
}
struct Frame {
    owner: Rc<()>,
    mode: Mode,
}
thread_local! { static ACTIVE: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) }; }
pub struct Scope {
    // Rc also keeps this scope bound to its creating thread.
    owner: Rc<()>,
}
impl Scope {
    pub fn observe() -> Self {
        Self::install(Mode::Observe {
            values: Vec::new(),
            complete: true,
        })
    }
    pub fn replay(values: Vec<(i64, u32)>) -> Result<Self, CelError> {
        if values.len() > CAP
            || values
                .iter()
                .any(|(s, n)| DateTime::from_timestamp(*s, *n).is_none())
        {
            return Err(unavailable());
        }
        Ok(Self::install(Mode::Replay {
            values,
            index: 0,
            complete: true,
        }))
    }
    fn install(mode: Mode) -> Self {
        let owner = Rc::new(());
        ACTIVE.with(|v| {
            v.borrow_mut().push(Frame {
                owner: Rc::clone(&owner),
                mode,
            })
        });
        Self { owner }
    }
    pub fn complete(&self) -> bool {
        ACTIVE.with(|v| {
            let frames = v.borrow();
            let frame = match frames.last() {
                Some(frame) if Rc::ptr_eq(&frame.owner, &self.owner) => frame,
                _ => return false,
            };
            match &frame.mode {
                Mode::Observe { complete, .. } => *complete,
                Mode::Replay {
                    values,
                    index,
                    complete,
                } => *complete && *index == values.len(),
            }
        })
    }
    pub fn finish(self) -> Option<Vec<(i64, u32)>> {
        ACTIVE.with(|v| match v.borrow().last() {
            Some(Frame {
                owner,
                mode:
                    Mode::Observe {
                        values,
                        complete: true,
                    },
            }) if Rc::ptr_eq(owner, &self.owner) => Some(values.clone()),
            _ => None,
        })
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|v| {
            let mut frames = v.borrow_mut();
            if let Some(index) = frames
                .iter()
                .position(|frame| Rc::ptr_eq(&frame.owner, &self.owner))
            {
                if index + 1 == frames.len() {
                    frames.pop();
                } else {
                    // A misnested lifetime invalidates active evidence. Stale
                    // guards must never restore a replay into later execution.
                    frames.clear();
                }
            }
        });
    }
}
fn unavailable() -> CelError {
    CelError::runtime("CEL clock provenance unavailable")
}
pub fn replaying() -> bool {
    ACTIVE.with(|v| {
        matches!(
            v.borrow().last().map(|frame| &frame.mode),
            Some(Mode::Replay { .. })
        )
    })
}
pub(crate) fn now() -> Result<DateTime<Utc>, CelError> {
    ACTIVE.with(|cell| {
        let mut frames = cell.borrow_mut();
        match frames.last_mut().map(|frame| &mut frame.mode) {
            Some(Mode::Replay {
                values,
                index,
                complete,
            }) => {
                let Some(&(s, n)) = values.get(*index) else {
                    *complete = false;
                    return Err(unavailable());
                };
                *index += 1;
                DateTime::from_timestamp(s, n).ok_or_else(unavailable)
            }
            mode => {
                let value = Utc::now();
                if let Some(Mode::Observe { values, complete }) = mode {
                    if values.len() < CAP {
                        values.push((value.timestamp(), value.timestamp_subsec_nanos()));
                    } else {
                        *complete = false;
                    }
                }
                Ok(value)
            }
        }
    })
}
