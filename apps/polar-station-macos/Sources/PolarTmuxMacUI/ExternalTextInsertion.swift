import AppKit
import ApplicationServices
import Carbon

@MainActor private func traceExternalInsertion(_ stage:String) {
    if ProcessInfo.processInfo.environment["POLAR_STATION_TRACE_DIR"] != nil {NSLog("Dictate external %@",stage)}
}

/// OS facts only. The Station reducer owns pending/delivered/recovery semantics.
public enum ExternalTextInsertionResult: String, Sendable { case verified, notPosted, postedUnconfirmed }

@MainActor
public final class ExternalTextInsertion {
    private static var clipboard: ExternalClipboardPaste?

    public static func cancel() { clipboard?.cancel(); clipboard=nil }

    @MainActor public final class Target {
        public let context: String
        private let pid:pid_t
        private let element:AXUIElement
        private let original:String?
        private let selection:NSRange?
        private let marker:CFTypeRef?
        private let preserveSelection:Bool
        private let emptyValue:Bool
        fileprivate init(pid:pid_t,element:AXUIElement,original:String?,selection:NSRange?,preserveSelection:Bool,emptyValue:Bool) {
            self.pid=pid;self.element=element;self.original=original;self.selection=selection
            self.marker=attribute(element,"AXSelectedTextMarkerRange")
            self.preserveSelection=preserveSelection;self.emptyValue=emptyValue
            if let original,let selection,NSMaxRange(selection)<=original.utf16.count {
                let source=original as NSString,start=max(0,selection.location-1024),end=min(source.length,NSMaxRange(selection)+1024)
                context=source.substring(with:NSRange(location:start,length:end-start))
            } else {context=""}
        }
        private func current()->Bool {
            guard NSWorkspace.shared.frontmostApplication?.processIdentifier==pid,
                  let focused=focusedElement(pid),CFEqual(focused,element),!protectedControl(element) else{return false}
            let value=textValue(element) ?? (emptyValue && (attribute(element,kAXNumberOfCharactersAttribute) as? NSNumber)?.intValue==0 ? "":nil)
            if let original,value != original{return false}
            let currentSelection=range(element) ?? (emptyValue ? NSRange(location:0,length:0):nil)
            if let selection,currentSelection != selection{return false}
            if let marker,let now=attribute(element,"AXSelectedTextMarkerRange"),!CFEqual(marker,now){return false}
            return true
        }
        public func insert(_ text:String,completion:@escaping (ExternalTextInsertionResult)->Void) {
            guard !text.isEmpty,current() else{traceExternalInsertion("not-posted captured target changed");completion(.notPosted);return}
            traceExternalInsertion("target admitted pid=\(pid) empty=\(emptyValue) range=\(selection != nil) units=\(text.utf16.count)")
            var insertion=selection
            if preserveSelection,let selection,selection.length>0 {
                var next=CFRange(location:NSMaxRange(selection),length:0)
                guard let value=AXValueCreate(.cfRange,&next),AXUIElementSetAttributeValue(element,kAXSelectedTextRangeAttribute as CFString,value) == .success else{completion(.notPosted);return}
                insertion=NSRange(location:next.location,length:0)
            }
            let expected:String?=original.flatMap { old in
                guard let insertion,NSMaxRange(insertion)<=old.utf16.count else{return nil}
                return (old as NSString).replacingCharacters(in:insertion,with:text)
            }
            // Preserve the verified empty Messages-field adaptation. Normal
            // external editors use their own native Paste command and Undo.
            if emptyValue {
                let result=AXUIElementSetAttributeValue(element,kAXValueAttribute as CFString,text as CFString)
                completion(result == .success && textValue(element)==text ? .verified : .postedUnconfirmed)
                return
            }
            guard ExternalTextInsertion.clipboard==nil else{completion(.notPosted);return}
            guard CGPreflightPostEventAccess(),let key=pasteKey() else{completion(.notPosted);return}
            let transfer=ExternalClipboardPaste(pasteboard:.general,text:text,confirmed:{[self] in
                guard NSWorkspace.shared.frontmostApplication?.processIdentifier==pid,
                      let focused=focusedElement(pid),CFEqual(focused,element),let expected else{return false}
                let value=textValue(element)
                traceExternalInsertion("readback expected-units=\(expected.utf16.count) actual-units=\(value?.utf16.count ?? -1)")
                return value==expected
            },beforePost:{[self] in
                guard NSWorkspace.shared.frontmostApplication?.processIdentifier==pid,
                      let focused=focusedElement(pid),CFEqual(focused,element),!protectedControl(element),
                      !IsSecureEventInputEnabled(),attribute(element,kAXEnabledAttribute) as? Bool != false else{return false}
                if let original,textValue(element) != original{return false}
                if let insertion,range(element) != insertion{return false}
                return true
            },post:{
                guard let source=CGEventSource(stateID:.privateState),
                      let commandDown=CGEvent(keyboardEventSource:source,virtualKey:CGKeyCode(kVK_Command),keyDown:true),
                      let down=CGEvent(keyboardEventSource:source,virtualKey:key,keyDown:true),
                      let up=CGEvent(keyboardEventSource:source,virtualKey:key,keyDown:false),
                      let commandUp=CGEvent(keyboardEventSource:source,virtualKey:CGKeyCode(kVK_Command),keyDown:false) else{return false}
                commandDown.flags = .maskCommand;down.flags = .maskCommand;up.flags = .maskCommand;commandUp.flags = []
                for event in [commandDown,down,up,commandUp] {event.post(tap:.cghidEventTap)}
                return true
            },completion:completion,released:{ExternalTextInsertion.clipboard=nil})
            ExternalTextInsertion.clipboard=transfer
            transfer.observe(pid:pid,element:element)
            transfer.start()
        }
    }

    public static func selectedText()->String {
        guard AXIsProcessTrusted(),let app=NSWorkspace.shared.frontmostApplication,
              app.processIdentifier != ProcessInfo.processInfo.processIdentifier,
              let element=focusedElement(app.processIdentifier),!protectedControl(element) else{return ""}
        return attribute(element,kAXSelectedTextAttribute) as? String ?? ""
    }
    public static func capture(activationPoint:CGPoint?=nil,preserveSelection:Bool=false)->Target? {
        clipboard?.relinquishIfOwnershipLost()
        traceExternalInsertion("capture permission=\(AXIsProcessTrusted()) secure=\(IsSecureEventInputEnabled())")
        guard AXIsProcessTrusted(),!IsSecureEventInputEnabled(),let app=NSWorkspace.shared.frontmostApplication,
              app.processIdentifier != ProcessInfo.processInfo.processIdentifier,
              let element=focusedElement(app.processIdentifier) else{return nil}
        AXUIElementSetMessagingTimeout(element,0.25)
        guard !protectedControl(element),attribute(element,kAXEnabledAttribute) as? Bool != false else{return nil}
        let role=attribute(element,kAXRoleAttribute) as? String
        let selection=range(element)
        traceExternalInsertion("focused role=\(role ?? "unknown") range=\(selection != nil)")
        let textRole=role==kAXTextFieldRole || role==kAXTextAreaRole || attribute(element,"AXEditable") as? Bool==true
        let textLayout=role=="AXLayoutArea" && (selection != nil || attribute(element,"AXSelectedTextMarkerRange") != nil)
        guard textRole || textLayout else{return nil}
        if let activationPoint,!hitEditor(element,at:activationPoint){traceExternalInsertion("capture rejected hit differs from focus");return nil}
        let value=textValue(element)
        let original=value.flatMap{$0.utf16.count<=65536 ? $0 : nil}
        let empty=(!settable(element,kAXSelectedTextAttribute)) && settable(element,kAXValueAttribute)
            && (original?.isEmpty==true || (attribute(element,kAXNumberOfCharactersAttribute) as? NSNumber)?.intValue==0)
            && (selection==nil || selection==NSRange(location:0,length:0))
        return Target(pid:app.processIdentifier,element:element,original:original ?? (empty ? "" : nil),selection:selection ?? (empty ? NSRange(location:0,length:0):nil),preserveSelection:preserveSelection,emptyValue:empty)
    }
    private static func attribute(_ element:AXUIElement,_ name:String)->CFTypeRef? {
        var value:CFTypeRef?
        return AXUIElementCopyAttributeValue(element,name as CFString,&value) == .success ? value:nil
    }
    private static func settable(_ element:AXUIElement,_ name:String)->Bool {
        var value=DarwinBoolean(false)
        return AXUIElementIsAttributeSettable(element,name as CFString,&value) == .success && value.boolValue
    }
    private static func focusedElement(_ pid:pid_t)->AXUIElement? {
        let app=AXUIElementCreateApplication(pid);AXUIElementSetMessagingTimeout(app,0.25)
        if let value=attribute(app,kAXFocusedUIElementAttribute),CFGetTypeID(value)==AXUIElementGetTypeID(){return unsafeDowncast(value,to:AXUIElement.self)}
        let system=AXUIElementCreateSystemWide();AXUIElementSetMessagingTimeout(system,0.25)
        guard let owner=attribute(system,kAXFocusedApplicationAttribute),CFGetTypeID(owner)==AXUIElementGetTypeID() else{return nil}
        var ownerPID:pid_t=0;AXUIElementGetPid(unsafeDowncast(owner,to:AXUIElement.self),&ownerPID)
        guard ownerPID==pid,let value=attribute(system,kAXFocusedUIElementAttribute),CFGetTypeID(value)==AXUIElementGetTypeID() else{return nil}
        return unsafeDowncast(value,to:AXUIElement.self)
    }
    private static func range(_ element:AXUIElement)->NSRange? {
        guard let value=attribute(element,kAXSelectedTextRangeAttribute),CFGetTypeID(value)==AXValueGetTypeID() else{return nil}
        let ax=unsafeDowncast(value,to:AXValue.self);var result=CFRange()
        guard AXValueGetType(ax) == .cfRange,AXValueGetValue(ax,.cfRange,&result),result.location>=0,result.length>=0 else{return nil}
        return NSRange(location:result.location,length:result.length)
    }
    private static func textValue(_ element:AXUIElement)->String? {
        let value=attribute(element,kAXValueAttribute)
        return (value as? String) ?? (value as? NSAttributedString)?.string
    }
    private static func protectedControl(_ element:AXUIElement)->Bool {
        var current:AXUIElement?=element
        for _ in 0..<12 {
            guard let node=current else{break}
            if attribute(node,kAXSubroleAttribute) as? String==kAXSecureTextFieldSubrole || attribute(node,"AXProtectedContent") as? Bool==true{return true}
            guard let parent=attribute(node,kAXParentAttribute),CFGetTypeID(parent)==AXUIElementGetTypeID() else{break}
            current=unsafeDowncast(parent,to:AXUIElement.self)
        }
        return false
    }
    private static func hitEditor(_ element:AXUIElement,at point:CGPoint)->Bool {
        let system=AXUIElementCreateSystemWide();AXUIElementSetMessagingTimeout(system,0.25)
        var hit:AXUIElement?
        guard AXUIElementCopyElementAtPosition(system,Float(point.x),Float(point.y),&hit) == .success else{return false}
        for _ in 0..<16 {
            guard let current=hit else{break}
            if CFEqual(current,element){return true}
            guard let parent=attribute(current,kAXParentAttribute),CFGetTypeID(parent)==AXUIElementGetTypeID() else{break}
            hit=unsafeDowncast(parent,to:AXUIElement.self)
        }
        return false
    }
    private static func pasteKey()->CGKeyCode? {
        guard let source=TISCopyCurrentKeyboardLayoutInputSource()?.takeRetainedValue(),
              let raw=TISGetInputSourceProperty(source,kTISPropertyUnicodeKeyLayoutData) else{return nil}
        let data=Unmanaged<CFData>.fromOpaque(raw).takeUnretainedValue()
        let layout=unsafeBitCast(CFDataGetBytePtr(data),to:UnsafePointer<UCKeyboardLayout>.self)
        for code:UInt16 in 0..<128 {
            var dead:UInt32=0,length=0;var characters=[UniChar](repeating:0,count:8)
            let status=UCKeyTranslate(layout,code,UInt16(kUCKeyActionDisplay),UInt32(cmdKey)>>8,UInt32(LMGetKbdType()),OptionBits(kUCKeyTranslateNoDeadKeysBit),&dead,characters.count,&length,&characters)
            if status==noErr,String(utf16CodeUnits:characters,count:length)=="v"{return CGKeyCode(code)}
        }
        return nil
    }

}

/// One materialized pasteboard item and its OS observations; no submission queue.
@MainActor
final class ExternalClipboardPaste:NSObject {
    private let pasteboard:NSPasteboard
    private let text:String
    private let confirmed:()->Bool
    private let beforePost:()->Bool
    private let post:()->Bool
    private var completion:((ExternalTextInsertionResult)->Void)?
    private let released:()->Void
    private var saved:[[(NSPasteboard.PasteboardType,Data)]]=[]
    private var ownership:Int?
    private var textReady=false
    private var valueChanged=false
    private var posted=false
    private var closed=false
    private var observer:AXObserver?
    private var deadline:Timer?
    init(pasteboard:NSPasteboard,text:String,confirmed:@escaping ()->Bool,beforePost:@escaping ()->Bool={true},post:@escaping ()->Bool,completion:@escaping (ExternalTextInsertionResult)->Void,released:@escaping ()->Void={}) {
        self.pasteboard=pasteboard;self.text=text;self.confirmed=confirmed;self.beforePost=beforePost;self.post=post;self.completion=completion;self.released=released
    }
    func observe(pid:pid_t,element:AXUIElement) {
        var value:AXObserver?
        guard AXObserverCreate(pid,{_,_,_,context in
            guard let context else{return}
            MainActor.assumeIsolated {Unmanaged<ExternalClipboardPaste>.fromOpaque(context).takeUnretainedValue().changed()}
        },&value) == .success,let value else{return}
        observer=value
        AXObserverAddNotification(value,element,kAXValueChangedNotification as CFString,Unmanaged.passUnretained(self).toOpaque())
        CFRunLoopAddSource(CFRunLoopGetMain(),AXObserverGetRunLoopSource(value),.commonModes)
    }
    func start() {
        let before=pasteboard.changeCount
        for old in pasteboard.pasteboardItems ?? [] {
            var representations:[(NSPasteboard.PasteboardType,Data)]=[]
            for type in old.types {
                guard let data=old.data(forType:type) else{finish(.notPosted);return}
                representations.append((type,data))
            }
            saved.append(representations)
        }
        guard pasteboard.changeCount==before else{finish(.notPosted);return}
        pasteboard.prepareForNewContents(with:.currentHostOnly)
        ownership=pasteboard.changeCount
        guard pasteboard.setString(text,forType:.string),
              pasteboard.setData(Data(),forType:NSPasteboard.PasteboardType("org.nspasteboard.TransientType")),
              pasteboard.setData(Data(),forType:NSPasteboard.PasteboardType("org.nspasteboard.ConcealedType")) else{finish(.notPosted);return}
        ownership=pasteboard.changeCount
        guard pasteboard.string(forType:.string)==text else{finish(.notPosted);return}
        traceExternalInsertion("published sequence=\(pasteboard.changeCount) units=\(text.utf16.count)")
        textReady=true
        postPrepared()
    }

    private func postPrepared() {
        guard !closed else{return}
        guard let ownership,pasteboard.changeCount==ownership else{finish(.notPosted,restore:false);return}
        let ready=beforePost()
        guard !closed else{return}
        guard ready else{finish(.notPosted);return}
        guard pasteboard.changeCount==ownership else{finish(.notPosted,restore:false);return}
        guard post() else{finish(.notPosted);return}
        posted=true
        traceExternalInsertion("paste posted")
        deadline=Timer.scheduledTimer(withTimeInterval:2,repeats:false){[weak self] _ in
            MainActor.assumeIsolated {self?.expire()}
        }
        assess()
    }
    func changed() {
        valueChanged=true
        assess()
    }
    private func assess() {
        guard !closed,posted else{return}
        if textReady && valueChanged && confirmed(){finish(.verified)}
        else if let ownership,pasteboard.changeCount != ownership{finish(.postedUnconfirmed,restore:false)}
    }
    func relinquishIfOwnershipLost() {
        if !closed,let ownership,pasteboard.changeCount != ownership {finish(posted ? .postedUnconfirmed:.notPosted,restore:false)}
    }
    func expire() {
        guard !closed else{return}
        if textReady && confirmed(){finish(.verified);return}
        // A consumer may still be about to read. Never restore old contents
        // ahead of it. Keep owned data until confirmed insertion or owner loss.
        let callback=completion;completion=nil;callback?(.postedUnconfirmed)
        deadline=nil
    }
    func cancel(){finish(posted ? .postedUnconfirmed:.notPosted)}
    private func finish(_ result:ExternalTextInsertionResult,restore:Bool=true) {
        guard !closed else{return};closed=true
        traceExternalInsertion("completion=\(result.rawValue) ready=\(textReady)")
        deadline?.invalidate();deadline=nil
        if let observer {CFRunLoopRemoveSource(CFRunLoopGetMain(),AXObserverGetRunLoopSource(observer),.commonModes)}
        observer=nil
        if restore,let ownership,pasteboard.changeCount==ownership {
            let restored=saved.map { types -> NSPasteboardItem in
                let item=NSPasteboardItem();for (type,data) in types{item.setData(data,forType:type)};return item
            }
            pasteboard.clearContents();if !restored.isEmpty{pasteboard.writeObjects(restored)}
        }
        let callback=completion;completion=nil
        released();callback?(result)
    }
}
