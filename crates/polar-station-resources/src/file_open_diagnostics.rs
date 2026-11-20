use crate::{FileOpenFailure, FileOpenStage, FileReferenceInput, TextRange};
use serde::Serialize;
use std::fmt::Write;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileOpenStep {
    pub title: &'static str,
    pub summary: String,
    pub details: Option<String>,
    pub failed: bool,
}

impl FileOpenStep {
    fn new(title: &'static str, observed: Option<String>, details: Option<String>) -> Self {
        Self {
            title,
            summary: observed.unwrap_or_else(|| "Not run".into()),
            details,
            failed: false,
        }
    }
}

pub(crate) fn summary(failure: &FileOpenFailure) -> &'static str {
    match failure.code.as_str() {
        "file_not_found" => "The file was not found at the path checked on the source machine.",
        "file_reference_missing" => "No complete file reference was recognized.",
        "session_directory_unavailable" => {
            "The original Session's working directory is unavailable."
        }
        "source_path_invalid" | "unresolved_source_path" => {
            "Could not resolve the recognized path on the source machine."
        }
        "desktop_file_unavailable" => {
            "The file exists on the source but is unavailable at the desktop path."
        }
        _ => match failure.stage {
            FileOpenStage::Capture => "Could not read the terminal reference.",
            FileOpenStage::Parse => "Could not interpret the file reference.",
            FileOpenStage::Source => "Could not resolve the original file source.",
            FileOpenStage::Verify => "Could not check the file on the source machine.",
            FileOpenStage::Access => "Could not access the file from this desktop.",
            FileOpenStage::Viewer => "The system application could not open the file.",
        },
    }
}

pub(crate) fn edit_range(input: &FileReferenceInput) -> Option<TextRange> {
    let FileReferenceInput::Recognized { recognized, .. } = input else {
        return None;
    };
    Some(TextRange {
        start: recognized.ranges.first()?.start,
        end: recognized.ranges.last()?.end,
    })
}

pub(crate) fn copy_details(steps: &[FileOpenStep]) -> String {
    steps
        .iter()
        .map(|step| {
            let mut text = format!("{}\n{}", step.title, step.summary);
            if let Some(details) = &step.details {
                let _ = write!(text, "\n{details}");
            }
            text
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(crate) fn steps(
    input: Option<&FileReferenceInput>,
    failure: &FileOpenFailure,
) -> Vec<FileOpenStep> {
    let mut trace = failure.recognition_trace.as_deref();
    let mut recognized = None;
    if let Some(FileReferenceInput::Recognized {
        recognized: value,
        trace: capture,
        ..
    }) = input
    {
        trace = trace.or(capture.as_deref());
        recognized = Some(value);
    }
    let text = failure
        .captured_text
        .as_deref()
        .or(input.map(FileReferenceInput::text));
    let captured = text.map(|text| {
        if let Some(trace) = trace {
            let mut detail = format!("Captured text:\n{text}\n\nHit byte: {}", trace.hit);
            for row in &trace.rows {
                let kind = if row.soft_wrap {
                    "soft wrap"
                } else {
                    "hard boundary"
                };
                let _ = write!(detail, "\nRow ends at byte {} ({kind})", row.end);
            }
            detail
        } else {
            text.to_owned()
        }
    });
    let joined = trace.and_then(|trace| trace.logical.as_ref());
    let context = failure.context.as_deref();
    let checked_path = context.and_then(|context| context.checked_path.as_ref());
    let checked = context.map(|context| {
        let target = &context.origin.execution_target_id;
        if let Some(path) = checked_path {
            let cwd = context
                .origin
                .cwd
                .as_deref()
                .unwrap_or("Not needed or unavailable");
            format!("Source machine: {target}\nWorking directory: {cwd}\nChecked path: {path}")
        } else {
            format!("Requested source machine: {target}")
        }
    });
    let mut rows = vec![
        FileOpenStep::new(
            "Captured text",
            text.map(|text| format!("{} physical row(s)", text.lines().count())),
            captured,
        ),
        FileOpenStep::new(
            "Line joining",
            joined
                .map(|value| {
                    if value.first_row == value.last_row {
                        format!("Used row {} without joining", value.first_row)
                    } else {
                        format!("Joined rows {}–{}", value.first_row, value.last_row)
                    }
                })
                .or_else(|| recognized.is_some().then(|| "Not recorded".into())),
            joined.map(|value| format!("{}\n\nLogical hit byte: {}", value.text, value.hit)),
        ),
        FileOpenStep::new(
            "Recognized reference",
            recognized.map(|value| {
                let mut path = value.reference.path.clone();
                if let Some(location) = &value.reference.location {
                    let _ = write!(path, ":{}", location.line);
                    if let Some(column) = location.column {
                        let _ = write!(path, ":{column}");
                    }
                }
                path
            }),
            recognized.map(|value| {
                value
                    .ranges
                    .iter()
                    .map(|range| format!("Source bytes {}–{}", range.start, range.end))
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
        ),
        FileOpenStep::new("Source file check", checked_path.cloned(), checked),
        FileOpenStep::new(
            "Desktop access and open",
            None,
            context
                .and_then(|context| context.local_path.as_ref())
                .map(|path| format!("Desktop path: {path}")),
        ),
    ];
    let index = match failure.stage {
        FileOpenStage::Capture => 0,
        FileOpenStage::Parse if recognized.is_some() => 3,
        FileOpenStage::Parse if trace.is_some_and(|trace| trace.logical.is_none()) => 1,
        FileOpenStage::Parse => 2,
        FileOpenStage::Source | FileOpenStage::Verify => 3,
        FileOpenStage::Access | FileOpenStage::Viewer => 4,
    };
    let failed = &mut rows[index];
    failed.failed = true;
    failed.summary = match (index, checked_path) {
        (3, Some(path)) => format!("{}\n{path}", summary(failure)),
        _ => summary(failure).into(),
    };
    let details = failed.details.get_or_insert_with(String::new);
    if !details.is_empty() {
        details.push_str("\n\n");
    }
    details.push_str(&failure.message);
    rows
}
