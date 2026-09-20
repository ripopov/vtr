//! Fixed compile-time column catalogues for the two baseline row domains.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionColumn {
    Label,
    Begin,
    Duration,
    End,
    Id,
    Status,
    Attributes,
}

impl TransactionColumn {
    pub const ALL: [Self; 7] = [
        Self::Label,
        Self::Begin,
        Self::Duration,
        Self::End,
        Self::Id,
        Self::Status,
        Self::Attributes,
    ];
    pub const DEFAULT: [Self; 5] = [
        Self::Label,
        Self::Begin,
        Self::Duration,
        Self::Status,
        Self::Attributes,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Label => "label",
            Self::Begin => "begin",
            Self::Duration => "duration",
            Self::End => "end",
            Self::Id => "id",
            Self::Status => "status",
            Self::Attributes => "attributes",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Label => "Label · vtr.label",
            Self::Begin => "Begin",
            Self::Duration => "Duration",
            Self::End => "End",
            Self::Id => "ID",
            Self::Status => "Status",
            Self::Attributes => "Attributes · preview",
        }
    }
    pub fn width(self) -> f32 {
        match self {
            Self::Label => 210.0,
            Self::Begin | Self::End => 120.0,
            Self::Duration => 105.0,
            Self::Id => 100.0,
            Self::Status => 90.0,
            Self::Attributes => 360.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ColumnSet {
    Transactions(Vec<TransactionColumn>),
    /// `true` entries correspond to the fixed captured signal order. Time is
    /// represented separately and can also be hidden, but at least one data
    /// column is always retained.
    Signals {
        time: bool,
        visible: Vec<bool>,
    },
}

impl ColumnSet {
    pub fn transactions_default() -> Self {
        Self::Transactions(TransactionColumn::DEFAULT.to_vec())
    }
    pub fn signals(count: usize) -> Self {
        Self::Signals {
            time: true,
            visible: vec![true; count],
        }
    }
    pub fn transaction_visible(&self, column: TransactionColumn) -> bool {
        matches!(self, Self::Transactions(v) if v.contains(&column))
    }
    pub fn toggle_transaction(&mut self, column: TransactionColumn) -> bool {
        let Self::Transactions(visible) = self else {
            return false;
        };
        if let Some(index) = visible.iter().position(|&c| c == column) {
            if visible.len() == 1 {
                return false;
            }
            visible.remove(index);
        } else {
            visible.push(column);
            visible.sort_by_key(|c| TransactionColumn::ALL.iter().position(|x| x == c));
        }
        true
    }
    pub fn reset(&mut self) {
        match self {
            Self::Transactions(v) => *v = TransactionColumn::DEFAULT.to_vec(),
            Self::Signals { time, visible } => {
                *time = true;
                visible.fill(true);
            }
        }
    }
}
