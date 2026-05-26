#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TransferRoute {
    Lan,
    Cloud,
}

impl TransferRoute {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Lan => "lan",
            Self::Cloud => "cloud",
        }
    }

    pub(super) fn from_str(route: &str) -> Option<Self> {
        match route {
            "lan" => Some(Self::Lan),
            "cloud" => Some(Self::Cloud),
            _ => None,
        }
    }
}
