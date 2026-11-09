use std::fmt;

pub const CONNECTOR_INLINE_ATTACHMENT_MAX_COUNT: usize = 3;
pub const CONNECTOR_INLINE_ATTACHMENT_MAX_BYTES: u64 = 1_048_576;
pub const CONNECTOR_INLINE_ATTACHMENT_TOTAL_MAX_BYTES: u64 =
    CONNECTOR_INLINE_ATTACHMENT_MAX_BYTES * CONNECTOR_INLINE_ATTACHMENT_MAX_COUNT as u64;
pub const CONNECTOR_INQUIRY_SUBMIT_ATTACHMENT_MAX_COUNT: usize = 1;
pub const CONNECTOR_INQUIRY_SUBMIT_ATTACHMENT_MAX_BYTES: u64 = 1_048_576;
pub const FAMILIAR_INQUIRY_SUBMIT_MIME_TYPE: &str =
    "application/vnd.polar.familiar-inquiry-submit+json";

const CUSTOMER_CHAT_KINDS: &[&str] = &["image", "text", "document"];
const CUSTOMER_CHAT_MIME_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "text/plain",
    "text/markdown",
    "text/csv",
    "application/json",
];
const FAMILIAR_INQUIRY_SUBMIT_KINDS: &[&str] = &["document"];
const FAMILIAR_INQUIRY_SUBMIT_MIME_TYPES: &[&str] =
    &[FAMILIAR_INQUIRY_SUBMIT_MIME_TYPE, "application/json"];
const NO_ATTACHMENT_KINDS: &[&str] = &[];
const NO_ATTACHMENT_MIME_TYPES: &[&str] = &[];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorAttachmentPolicy {
    pub max_count: usize,
    pub inline_max_bytes: u64,
    pub inline_total_max_bytes: u64,
    pub allowed_kinds: &'static [&'static str],
    pub allowed_mime_types: &'static [&'static str],
}

impl ConnectorAttachmentPolicy {
    pub const fn new(
        max_count: usize,
        inline_max_bytes: u64,
        inline_total_max_bytes: u64,
        allowed_kinds: &'static [&'static str],
        allowed_mime_types: &'static [&'static str],
    ) -> Self {
        Self {
            max_count,
            inline_max_bytes,
            inline_total_max_bytes,
            allowed_kinds,
            allowed_mime_types,
        }
    }

    pub const fn customer_chat_inline() -> Self {
        Self::new(
            CONNECTOR_INLINE_ATTACHMENT_MAX_COUNT,
            CONNECTOR_INLINE_ATTACHMENT_MAX_BYTES,
            CONNECTOR_INLINE_ATTACHMENT_TOTAL_MAX_BYTES,
            CUSTOMER_CHAT_KINDS,
            CUSTOMER_CHAT_MIME_TYPES,
        )
    }

    pub const fn familiar_inquiry_submit() -> Self {
        Self::new(
            CONNECTOR_INQUIRY_SUBMIT_ATTACHMENT_MAX_COUNT,
            CONNECTOR_INQUIRY_SUBMIT_ATTACHMENT_MAX_BYTES,
            CONNECTOR_INQUIRY_SUBMIT_ATTACHMENT_MAX_BYTES,
            FAMILIAR_INQUIRY_SUBMIT_KINDS,
            FAMILIAR_INQUIRY_SUBMIT_MIME_TYPES,
        )
    }

    pub const fn no_attachments() -> Self {
        Self::new(0, 0, 0, NO_ATTACHMENT_KINDS, NO_ATTACHMENT_MIME_TYPES)
    }
}

impl ConnectorAttachmentInput for polar_lgp_model::InlineMessageAttachment {
    fn kind(&self) -> &str {
        &self.kind
    }

    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    fn mime_type(&self) -> &str {
        &self.mime_type
    }

    fn data_url(&self) -> &str {
        &self.data_url
    }
}

pub trait ConnectorAttachmentInput {
    fn kind(&self) -> &str;
    fn name(&self) -> Option<&str>;
    fn mime_type(&self) -> &str;
    fn data_url(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorAttachmentValidation {
    pub count: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorAttachmentValidationErrorKind {
    TooMany,
    MissingKind,
    MissingMimeType,
    MissingDataUrl,
    UnsupportedKind,
    UnsupportedMimeType,
    InvalidDataUrl,
    MimeTypeMismatch,
    TooLarge,
    TotalTooLarge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorAttachmentValidationError {
    pub kind: ConnectorAttachmentValidationErrorKind,
    pub code: &'static str,
    pub message: String,
}

impl ConnectorAttachmentValidationError {
    fn new(
        kind: ConnectorAttachmentValidationErrorKind,
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for ConnectorAttachmentValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConnectorAttachmentValidationError {}

pub fn validate_connector_thread_attachments<A: ConnectorAttachmentInput>(
    attachments: &[A],
    policy: &ConnectorAttachmentPolicy,
) -> Result<ConnectorAttachmentValidation, ConnectorAttachmentValidationError> {
    if attachments.len() > policy.max_count {
        return Err(ConnectorAttachmentValidationError::new(
            ConnectorAttachmentValidationErrorKind::TooMany,
            "connector_attachment_count_exceeded",
            format!(
                "connector thread message includes {} attachments; maximum is {}",
                attachments.len(),
                policy.max_count
            ),
        ));
    }

    let mut total_bytes = 0_u64;
    for (index, attachment) in attachments.iter().enumerate() {
        let display_index = index + 1;
        let kind = attachment.kind().trim();
        if kind.is_empty() {
            return Err(ConnectorAttachmentValidationError::new(
                ConnectorAttachmentValidationErrorKind::MissingKind,
                "connector_attachment_kind_missing",
                format!("connector attachment {display_index} is missing kind"),
            ));
        }
        if !policy
            .allowed_kinds
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(kind))
        {
            return Err(ConnectorAttachmentValidationError::new(
                ConnectorAttachmentValidationErrorKind::UnsupportedKind,
                "connector_attachment_kind_unsupported",
                format!("connector attachment {display_index} kind is not supported"),
            ));
        }

        let mime_type = attachment.mime_type().trim();
        if mime_type.is_empty() {
            return Err(ConnectorAttachmentValidationError::new(
                ConnectorAttachmentValidationErrorKind::MissingMimeType,
                "connector_attachment_mime_type_missing",
                format!("connector attachment {display_index} is missing mime_type"),
            ));
        }
        if !policy
            .allowed_mime_types
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(mime_type))
        {
            return Err(ConnectorAttachmentValidationError::new(
                ConnectorAttachmentValidationErrorKind::UnsupportedMimeType,
                "connector_attachment_mime_type_unsupported",
                format!("connector attachment {display_index} mime_type is not supported"),
            ));
        }

        let data_url = attachment.data_url().trim();
        if data_url.is_empty() {
            return Err(ConnectorAttachmentValidationError::new(
                ConnectorAttachmentValidationErrorKind::MissingDataUrl,
                "connector_attachment_data_url_missing",
                format!("connector attachment {display_index} is missing data_url"),
            ));
        }
        let Some(data_url_mime_type) = data_url_mime_type(data_url) else {
            return Err(ConnectorAttachmentValidationError::new(
                ConnectorAttachmentValidationErrorKind::InvalidDataUrl,
                "connector_attachment_data_url_invalid",
                format!("connector attachment {display_index} data_url is invalid"),
            ));
        };
        if !data_url_mime_type.eq_ignore_ascii_case(mime_type) {
            return Err(ConnectorAttachmentValidationError::new(
                ConnectorAttachmentValidationErrorKind::MimeTypeMismatch,
                "connector_attachment_mime_type_mismatch",
                format!(
                    "connector attachment {display_index} data_url MIME does not match mime_type"
                ),
            ));
        }
        let Some(size_bytes) = estimate_data_url_size_bytes(data_url) else {
            return Err(ConnectorAttachmentValidationError::new(
                ConnectorAttachmentValidationErrorKind::InvalidDataUrl,
                "connector_attachment_data_url_invalid",
                format!("connector attachment {display_index} data_url is invalid"),
            ));
        };
        if size_bytes > policy.inline_max_bytes {
            return Err(ConnectorAttachmentValidationError::new(
                ConnectorAttachmentValidationErrorKind::TooLarge,
                "connector_attachment_inline_bytes_exceeded",
                format!(
                    "connector attachment {display_index} is {size_bytes} bytes; maximum is {} bytes",
                    policy.inline_max_bytes
                ),
            ));
        }
        total_bytes = total_bytes.saturating_add(size_bytes);
        if total_bytes > policy.inline_total_max_bytes {
            return Err(ConnectorAttachmentValidationError::new(
                ConnectorAttachmentValidationErrorKind::TotalTooLarge,
                "connector_attachment_inline_total_bytes_exceeded",
                format!(
                    "connector attachments total {total_bytes} bytes; maximum is {} bytes",
                    policy.inline_total_max_bytes
                ),
            ));
        }
    }

    Ok(ConnectorAttachmentValidation {
        count: attachments.len(),
        total_bytes,
    })
}

pub fn estimate_data_url_size_bytes(data_url: &str) -> Option<u64> {
    let (_, encoded) = data_url.split_once(',')?;
    let encoded = encoded.trim();
    let normalized_len = encoded.len() as u64;
    let padding = encoded
        .as_bytes()
        .iter()
        .rev()
        .take_while(|byte| **byte == b'=')
        .count() as u64;
    Some(
        (normalized_len / 4)
            .saturating_mul(3)
            .saturating_sub(padding),
    )
}

pub fn data_url_mime_type(data_url: &str) -> Option<String> {
    let (header, _) = data_url.split_once(',')?;
    let header = header.strip_prefix("data:")?;
    let mut parts = header.split(';');
    let mime_type = parts.next()?.trim();
    if mime_type.is_empty() || !parts.any(|part| part.eq_ignore_ascii_case("base64")) {
        return None;
    }
    Some(mime_type.to_ascii_lowercase())
}


