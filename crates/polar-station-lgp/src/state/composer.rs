//! Projection of native selection; observing a Pane never grants keyboard focus.
use super::*;
impl Engine {
    pub(super) fn observe_composer_selection(&mut self, pane: &str, out: &mut Vec<Effect>) -> Result<(), String> {
        let window = self.views.get(pane).map(|v|v.window.clone())
            .or_else(||self.ideas.get(pane).map(|v|v.window.clone())).ok_or("unknown composer Pane")?;
        if self.selected.get(&window).is_none_or(|id|id!=pane) {
            self.inquiry_selected(pane);
            self.selected.insert(window,pane.into());
        }
        if let Some(view)=self.views.get(pane) {
            let scope=view.scope.clone();
            if self.ready {
                if self.threads.get(&scope).is_some_and(|t|matches!(t.status.as_str(),"offline"|"unavailable")) { self.bind(&scope,out)?; }
            } else {out.push(Effect::ReconnectHost);}
        }
        Ok(())
    }
    pub(super) fn activate_composer(&self, pane: &str, out: &mut Vec<Effect>) {
        let Some(view)=self.views.get(pane) else {return};
        if self.selected.get(&view.window).is_some_and(|id|id==pane) {
            out.push(Effect::FocusComposer{window_id:view.window.clone(),pane_id:pane.into()});
        }
    }
    pub(super) fn clear_composer_selection(&mut self,window:String,out:&mut Vec<Effect>) {
        if self.selected.remove(&window).is_none() {return;}
        self.composer_cache.remove(&window);
        out.push(Effect::ComposerClosed{window_id:window});
    }
}


