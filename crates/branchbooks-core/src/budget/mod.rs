//! Envelope budget engine. The core state (`BudgetState`) follows reference/sim4.py: split,
//! pay, unassigned, assign, move and rollover mean what they mean there. `BudgetEngine` adds
//! ledger-driven events, same-day ordering (rule 14), month views and incremental recompute.
//!
//! Events carry an id, so `remove` finds the event it was given.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use indexmap::{IndexMap, IndexSet};

use crate::date::{Date, Month};
use crate::ledger::Accounts;
use crate::loader::Options;
use crate::money::{Decimal, ZERO, fixed};
use crate::types::{Custom, CustomValue, Diagnostic, Entry, MetaValue};
use rust_decimal::RoundingStrategy;

mod config;
mod engine;
mod events;
mod state;
mod view;

pub use config::*;
pub use engine::*;
pub use events::*;
pub use state::*;
pub use view::*;
