use std::fmt::Write;

use serde::{Deserialize, Serialize};

use crate::error::{WorkspaceError, WorkspaceErrorCode};
use crate::ids::ManagedPaneId;
use crate::model::{DisplayMode, WorkspaceTab};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ratio {
    pub numerator: u32,
    pub denominator: u32,
}

impl Ratio {
    pub fn new(numerator: u32, denominator: u32) -> Result<Self, WorkspaceError> {
        if denominator == 0 {
            return Err(WorkspaceError::new(
                WorkspaceErrorCode::InvalidCatalog,
                "ratio denominator cannot be zero",
            ));
        }
        let divisor = greatest_common_divisor(numerator, denominator);
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    pub fn multiply(self, other: Self) -> Self {
        Self::new(
            self.numerator.saturating_mul(other.numerator),
            self.denominator.saturating_mul(other.denominator),
        )
        .expect("multiplying valid ratios keeps a non-zero denominator")
    }
}

const fn greatest_common_divisor(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    if left == 0 {
        1
    } else {
        left
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LayoutFingerprint(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutRole {
    Monitor,
    Main,
    Sidebar { slot: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutSlot {
    pub pane_id: ManagedPaneId,
    pub canonical_index: usize,
    pub visual_index: usize,
    pub column: usize,
    pub row: usize,
    pub width: Ratio,
    pub height: Ratio,
    pub role: LayoutRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalLayout {
    pub mode: DisplayMode,
    pub canonical_pane_order: Vec<ManagedPaneId>,
    pub slots: Vec<LayoutSlot>,
    pub main_pane_id: Option<ManagedPaneId>,
    pub sidebar_pane_order: Vec<ManagedPaneId>,
    pub column_count: usize,
    pub sidebar_column_count: usize,
    pub fingerprint: LayoutFingerprint,
}

impl CanonicalLayout {
    pub fn column_populations(&self) -> Vec<usize> {
        let mut populations = vec![0; self.column_count];
        for slot in &self.slots {
            if slot.role == LayoutRole::Monitor {
                populations[slot.column] += 1;
            }
        }
        populations
    }
}

pub fn canonical_layout(tab: &WorkspaceTab) -> Result<CanonicalLayout, WorkspaceError> {
    let canonical_pane_order: Vec<_> = tab.panes.iter().map(|pane| pane.id).collect();
    let mut layout = match tab.display_mode {
        DisplayMode::Monitor => monitor_layout(canonical_pane_order),
        DisplayMode::Focus => focus_layout(canonical_pane_order, tab.focused_pane_id)?,
    };
    layout.fingerprint = fingerprint(&layout);
    Ok(layout)
}

fn monitor_layout(canonical_pane_order: Vec<ManagedPaneId>) -> CanonicalLayout {
    let pane_count = canonical_pane_order.len();
    let column_count = match pane_count {
        0 => 0,
        1 => 1,
        2..=6 => 2,
        _ => 3,
    };
    let mut populations = vec![0usize; column_count];
    for index in 0..pane_count {
        populations[index % column_count] += 1;
    }
    let slots = canonical_pane_order
        .iter()
        .enumerate()
        .map(|(index, pane_id)| {
            let column = index % column_count;
            let row = index / column_count;
            LayoutSlot {
                pane_id: *pane_id,
                canonical_index: index,
                visual_index: index,
                column,
                row,
                width: Ratio::new(populations[column] as u32, pane_count as u32)
                    .expect("non-empty Monitor layout has a valid width"),
                height: Ratio::new(1, populations[column] as u32)
                    .expect("populated column has a valid height"),
                role: LayoutRole::Monitor,
            }
        })
        .collect();
    CanonicalLayout {
        mode: DisplayMode::Monitor,
        canonical_pane_order,
        slots,
        main_pane_id: None,
        sidebar_pane_order: Vec::new(),
        column_count,
        sidebar_column_count: 0,
        fingerprint: LayoutFingerprint(String::new()),
    }
}

fn focus_layout(
    canonical_pane_order: Vec<ManagedPaneId>,
    focused_pane_id: Option<ManagedPaneId>,
) -> Result<CanonicalLayout, WorkspaceError> {
    let focused_pane_id = focused_pane_id.ok_or_else(|| {
        WorkspaceError::new(
            WorkspaceErrorCode::FocusInvalid,
            "Focus mode requires a focused pane",
        )
    })?;
    let main_index = canonical_pane_order
        .iter()
        .position(|pane_id| *pane_id == focused_pane_id)
        .ok_or_else(|| {
            WorkspaceError::new(
                WorkspaceErrorCode::FocusInvalid,
                "focused pane is not in canonical pane order",
            )
        })?;
    let sidebar_pane_order: Vec<_> = canonical_pane_order
        .iter()
        .copied()
        .filter(|pane_id| *pane_id != focused_pane_id)
        .collect();
    let sidebar_count = sidebar_pane_order.len();
    let sidebar_column_count = match sidebar_count {
        0 => 0,
        1..=4 => 1,
        _ => 2,
    };
    let (main_width, combined_sidebar_width) = if sidebar_count == 0 {
        (
            Ratio::new(1, 1).expect("ratio"),
            Ratio::new(0, 1).expect("ratio"),
        )
    } else if sidebar_count <= 4 {
        (
            Ratio::new(4, 5).expect("ratio"),
            Ratio::new(1, 5).expect("ratio"),
        )
    } else {
        (
            Ratio::new(3, 5).expect("ratio"),
            Ratio::new(2, 5).expect("ratio"),
        )
    };
    let mut slots = vec![LayoutSlot {
        pane_id: focused_pane_id,
        canonical_index: main_index,
        visual_index: 0,
        column: 0,
        row: 0,
        width: main_width,
        height: Ratio::new(1, 1).expect("ratio"),
        role: LayoutRole::Main,
    }];
    let mut populations = vec![0usize; sidebar_column_count];
    for index in 0..sidebar_count {
        populations[index % sidebar_column_count] += 1;
    }
    for (index, pane_id) in sidebar_pane_order.iter().enumerate() {
        let column = index % sidebar_column_count;
        let row = index / sidebar_column_count;
        let column_fraction = Ratio::new(1, sidebar_column_count as u32).expect("ratio");
        slots.push(LayoutSlot {
            pane_id: *pane_id,
            canonical_index: canonical_pane_order
                .iter()
                .position(|candidate| candidate == pane_id)
                .expect("sidebar pane comes from canonical order"),
            visual_index: index + 1,
            column: column + 1,
            row,
            width: combined_sidebar_width.multiply(column_fraction),
            height: Ratio::new(1, populations[column] as u32).expect("ratio"),
            role: LayoutRole::Sidebar {
                slot: u8::try_from(index + 1).expect("at most eight sidebar panes"),
            },
        });
    }
    Ok(CanonicalLayout {
        mode: DisplayMode::Focus,
        canonical_pane_order,
        slots,
        main_pane_id: Some(focused_pane_id),
        sidebar_pane_order,
        column_count: if sidebar_count == 0 {
            1
        } else {
            1 + sidebar_column_count
        },
        sidebar_column_count,
        fingerprint: LayoutFingerprint(String::new()),
    })
}

fn fingerprint(layout: &CanonicalLayout) -> LayoutFingerprint {
    let mut value = format!("{:?}|", layout.mode);
    for slot in &layout.slots {
        let _ = write!(
            value,
            "{}:{}:{}:{}/{}:{}/{}:{:?}|",
            slot.pane_id,
            slot.column,
            slot.row,
            slot.width.numerator,
            slot.width.denominator,
            slot.height.numerator,
            slot.height.denominator,
            slot.role
        );
    }
    LayoutFingerprint(value)
}
