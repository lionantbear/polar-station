import PolarTmuxHostClient
import SwiftUI

/// One narrow Settings column. Editors keep their original domain owners.
public struct SettingsPane: View {
    @ObservedObject private var navigation: SettingsNavigation
    private let content: (SettingsNavigation.Route) -> AnyView
    public init(navigation: SettingsNavigation, content: @escaping (SettingsNavigation.Route) -> AnyView) {
        self.navigation = navigation; self.content = content
    }
    public var body: some View {
        VStack(spacing:0) {
            if let state = navigation.state {
                HStack {
                    if state.can_back {
                        Button(action:navigation.back) { Image(systemName:"chevron.left") }.help(state.back_label).accessibilityLabel(state.back_label)
                    }
                    Text(state.title).font(.headline)
                    Spacer(minLength:0)
                }.padding(12)
                Divider()
                if state.is_directory {
                    ScrollView {
                        VStack(spacing:0) {
                            ForEach(state.entries) { entry in
                                Button { navigation.navigate(entry.route) } label: {
                                    HStack(alignment:.top,spacing:10) {
                                        Image(systemName:entry.symbol).frame(width:20)
                                        VStack(alignment:.leading,spacing:4) {
                                            Text(entry.title).font(.headline)
                                            Text(entry.subtitle).font(.caption).foregroundStyle(.secondary)
                                        }.frame(maxWidth:.infinity,alignment:.leading)
                                        Image(systemName:"chevron.right").foregroundStyle(.secondary)
                                    }.padding(12).contentShape(Rectangle())
                                }.buttonStyle(.plain)
                                    .accessibilityLabel(entry.title)
                                    .help(entry.subtitle)
                                Divider().padding(.horizontal,12)
                            }
                        }
                    }
                } else {
                    if state.route.page == "setup" {
                        Picker("Machine scope",selection:Binding(get:{state.route.remote ?? false},set:{navigation.open(.init(page:"setup",remote:$0))})) {
                            ForEach(state.scopes) { scope in Text(scope.title).tag(scope.route.remote ?? false) }
                        }.pickerStyle(.segmented).padding(.horizontal,12)
                    }
                    content(state.route).frame(maxWidth:.infinity,maxHeight:.infinity)
                }
            }
        }.frame(maxWidth:.infinity,maxHeight:.infinity)
    }
}
