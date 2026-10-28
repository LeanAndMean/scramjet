use super::super::{CrosstermBackend, Rect, Terminal, TerminalOptions, Viewport, draw};
use super::*;
use scramjet_rs::{EditIntent, RequestId, RootIdentity};

#[test]
fn submission_requires_painted_current_revision_and_retains_draft_on_every_receipt() {
    let (bridge, _) = crate::coding_ui_demo::Bridge::new();
    let mut model = Model::runtime();
    model.editor.clear();
    model.caret = 0;
    let display = model.project(100, 24);
    model.displayed(display);
    let id = bridge.snapshot().offer.unwrap();
    let first = Some(crate::coding_ui_demo::Offer {
        id,
        revision: model.revision,
    });
    model.event(Event::Text("\n".into()));
    model.submit(&bridge, first, false, 100, 24);
    assert!(bridge.snapshot().submission.is_none());
    assert_eq!(model.editor, "\n");
    model.event(Event::Paste("a\r\n界\tb".into()));
    assert_eq!(model.editor, "\na\n界    b");
    let offer = Some(crate::coding_ui_demo::Offer {
        id,
        revision: model.revision,
    });
    bridge.painted(offer);
    let revision = model.revision;
    model.event(Event::Left);
    model.event(Event::Unknown);
    model.event(Event::Submit);
    assert_eq!(model.revision, revision);
    let draft = model.editor.clone();
    let caret = model.caret;
    for mode in 0..6 {
        model.focused = mode != 0;
        model.focus = if mode == 1 {
            Focus::Transcript
        } else {
            Focus::Editor
        };
        model.anchor = if mode == 2 { Some((0, 0)) } else { None };
        model.submit(
            &bridge,
            offer,
            mode == 3,
            if mode == 4 { 79 } else { 100 },
            if mode == 5 { 25 } else { 24 },
        );
        assert!(bridge.snapshot().submission.is_none());
        assert_eq!((&model.editor, model.caret), (&draft, caret));
    }
    model.focused = true;
    model.focus = Focus::Editor;
    model.anchor = None;
    model.insert("x");
    model.submit(&bridge, offer, false, 100, 24);
    assert!(
        bridge.snapshot().submission.is_none(),
        "same-batch edit needs another paint"
    );
    let offer = Some(crate::coding_ui_demo::Offer {
        id,
        revision: model.revision,
    });
    bridge.painted(offer);
    let submitted = model.editor.clone();
    let caret = model.caret;
    model.submit(&bridge, offer, false, 100, 24);
    assert!(bridge.snapshot().submission.is_some());
    assert_eq!((&model.editor, model.caret), (&submitted, caret));
    model.insert("late");
    model.submit(&bridge, offer, false, 100, 24);
    assert_eq!(bridge.snapshot().submission.unwrap().bytes, submitted.len());
    let before = model.revision;
    model.insert("");
    model.event(Event::Delete);
    assert!(model.revision >= before);
    model.event(Event::Backspace);
    assert!(model.revision > before);
}
fn proposal(text: &str) -> Arc<CodingProposal> {
    Arc::new(CodingProposal {
        owner: 1,
        invocation: 1,
        request_id: RequestId(2),
        turn: 2,
        intent: EditIntent {
            session: "s".into(),
            root: RootIdentity {
                device: 1,
                inode: 1,
            },
            operation: 1,
            target: "sample.txt".into(),
            expected: text.into(),
            replacement: text.into(),
        },
    })
}
fn decode(encoded: &str) -> String {
    let mut chars = encoded.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next().unwrap() {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            'u' => {
                assert_eq!(chars.next(), Some('{'));
                let mut hex = String::new();
                loop {
                    let c = chars.next().unwrap();
                    if c == '}' {
                        break;
                    }
                    hex.push(c);
                }
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap());
            }
            _ => panic!("invalid escape"),
        }
    }
    out
}
#[test]
fn every_scalar_and_literal_escape_newline_tab_bidi_are_reversible_ascii() {
    for scalar in 0..=0x10ffff {
        if let Some(c) = char::from_u32(scalar) {
            let input = c.to_string();
            let encoded = encode(&input);
            assert!(encoded.bytes().all(|b| (32..=126).contains(&b)));
            assert_eq!(decode(&encoded), input);
            assert!(encoded.len() <= input.len() * 12);
        }
    }
    let inputs = [
        "",
        " ",
        "x ",
        "x\n",
        "x\t",
        "\\n",
        "\\u{202E}",
        "\u{202e}",
        "e\u{301}",
        "👩‍💻",
        "\"\\",
    ];
    let encodings: std::collections::HashSet<_> = inputs.iter().map(|s| encode(s)).collect();
    assert_eq!(encodings.len(), inputs.len());
}
fn paint_page(approval: &mut Approval, model: &Model, width: u16, height: u16) -> Buffer {
    let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
    let complete = approval.paint(model, &mut buffer);
    approval.painted(complete);
    buffer
}
#[test]
fn complete_pages_and_focused_current_frame_are_required() {
    let mut model = Model::runtime();
    model.focus = Focus::Transcript;
    let mut approval = Approval::default();
    approval.observe(Some(proposal(&"\t".repeat(1024))));
    let mut reconstructed = String::new();
    assert!(
        approval.pages() > 1 && approval.pages() <= 64 && approval.document.len() <= DOCUMENT_LIMIT
    );
    assert!(
        approval
            .event(&Event::Approve, &model, &None, 80, 24)
            .is_none()
    );
    for page in 0..approval.pages() {
        approval.page = page;
        let buffer = paint_page(&mut approval, &model, 80, 24);
        let start = page * 512;
        for (line, chunk) in approval.document.as_bytes()
            [start..(start + 512).min(approval.document.len())]
            .chunks(64)
            .enumerate()
        {
            let cells: String = (6..6 + chunk.len())
                .map(|x| buffer[(x as u16, line as u16 + 2)].symbol())
                .collect();
            assert_eq!(cells.as_bytes(), chunk);
            reconstructed.push_str(&cells);
        }
        if page + 1 < approval.pages() {
            assert!(!approval.armed);
        }
    }
    assert_eq!(reconstructed, approval.document);
    assert!(approval.armed);
    let ingress = approval.displayed.clone();
    model.focused = false;
    assert!(
        approval
            .event(&Event::Approve, &model, &ingress, 80, 24)
            .is_none()
    );
    model.focused = true;
    paint_page(&mut approval, &model, 79, 24);
    assert!(!approval.armed);
    paint_page(&mut approval, &model, 80, 23);
    assert!(!approval.armed);
    paint_page(&mut approval, &model, 80, 24);
    assert!(
        approval
            .event(&Event::Approve, &model, &ingress, 80, 24)
            .is_some()
    );
    let mut changed = (*proposal("new")).clone();
    changed.owner = 2;
    approval.observe(Some(Arc::new(changed)));
    assert_eq!(approval.coverage, 0);
    assert!(!approval.armed);
    paint_page(&mut approval, &model, 80, 24);
    assert!(
        approval
            .event(&Event::Approve, &model, &ingress, 80, 24)
            .is_none()
    );
}
struct Fault;
impl std::io::Write for Fault {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("draw fault"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[test]
fn failed_output_cannot_acknowledge_page_or_arm_decision() {
    let mut model = Model::runtime();
    model.focus = Focus::Transcript;
    let mut approval = Approval::default();
    approval.observe(Some(proposal("x")));
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(Fault),
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, 80, 24)),
        },
    )
    .unwrap();
    let display = model.project(80, 24);
    assert!(
        draw(
            &mut terminal,
            &mut model,
            &mut None,
            display,
            Some(&mut approval),
            None,
            None
        )
        .is_err()
    );
    assert_eq!(approval.coverage, 0);
    assert!(!approval.armed && approval.displayed.is_none());
    let _ = terminal.show_cursor();
}
