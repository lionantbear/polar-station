//! Native label measurement only; the original models/factories still own choices.
const gtk = @import("gtk");
const gobject = @import("gobject");

pub fn wrapCaption(child: ?*gtk.Widget) void {
    if (child) |widget| if (gobject.ext.cast(gtk.Label, widget.as(gobject.Object))) |caption| {
        caption.setWrap(1);
        caption.setWrapMode(.word_char);
    };
}

/// GTK 4.22's default StringList factory binds an unwrapped Label inside a Box.
/// Run after that bind; retain its string/expression mapping and selected checkmark.
pub fn wrapDropDown(control: *gtk.DropDown) void {
    const base = control.getFactory() orelse return;
    const factory = gobject.ext.cast(gtk.SignalListItemFactory, base.as(gobject.Object)) orelse return;
    _ = gtk.SignalListItemFactory.signals.bind.connect(factory, ?*anyopaque, captionBound, null, .{});
}
fn captionBound(_: *gtk.SignalListItemFactory, object: *gobject.Object, _: ?*anyopaque) callconv(.c) void {
    const item = gobject.ext.cast(gtk.ListItem, object) orelse return;
    const box = item.getChild() orelse return;
    wrapCaption(box.getFirstChild());
}

/// CheckButton.getChild() intentionally returns null for its built-in label.
/// Preserve that original label and its mnemonic/accessibility relationship.
pub fn wrapCheck(control:*gtk.CheckButton) void {
    var child=control.as(gtk.Widget).getFirstChild();
    while(child) |widget| : (child=widget.getNextSibling()) wrapCaption(widget);
}
