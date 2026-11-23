//! The unfolded panel keeps the recovery menu beside the active page.
//! Only the viewport and its gestures change; both panels use one App.

use super::*;

const SEPARATOR: u32 = 1;

enum MenuDrag {
    Press { y: f32, action: Option<Action> },
    Scroll { last: f32 },
}

struct MenuArt {
    big: Blurs,
    small: Blurs,
    search: Canvas,
}

struct MenuHeader {
    mask: Fade,
    title: Fade,
    painted: (f32, f32),
}

impl MenuHeader {
    fn new(mask: bool, title: bool) -> Self {
        let (mut mask_fade, mut title_fade) = (bar_fade(false), title_fade(false));
        mask_fade.jump(mask);
        title_fade.jump(title);
        Self { mask: mask_fade, title: title_fade, painted: (0.0, 0.0) }
    }

    fn levels(&self, now: u64) -> (f32, f32) {
        (self.mask.level(now), self.title.level(now))
    }
}

#[derive(Default)]
pub(super) struct Menu {
    width: u32,
    scroll: f32,
    max_scroll: f32,
    /// Home is the full menu on the cover and the About overview on the
    /// inner display. Keep the two scroll positions separately.
    other_home_scroll: f32,
    frame: Option<Frame>,
    art: Option<MenuArt>,
    drag: Option<MenuDrag>,
    tracker: Tracker,
    fling: Option<Fling>,
    pull: f32,
    bounce: Option<Spring>,
    header: Option<MenuHeader>,
    pub(super) detail: Option<Canvas>,
    pub(super) charge_dither: Option<glow::Dither>,
}

impl Menu {
    pub(super) fn rescale(&mut self, scale: f32) {
        self.scroll *= scale;
        self.other_home_scroll *= scale;
    }

    pub(super) fn invalidate(&mut self) {
        self.frame = None;
        self.art = None;
    }

    pub(super) fn cancel(&mut self) {
        self.drag = None;
        self.fling = None;
        self.pull = 0.0;
        self.bounce = None;
        self.tracker.clear();
    }

    pub(super) fn reflow(&mut self) {
        self.invalidate();
        self.cancel();
        self.header = None;
        self.detail = None;
        self.charge_dither = None;
    }

    pub(super) fn moving(&self, now: u64) -> bool {
        self.fling.is_some() || self.bounce.is_some() || self.header_needs_draw(now)
    }

    fn header_needs_draw(&self, now: u64) -> bool {
        self.header.as_ref().is_some_and(|h| h.mask.moving(now) || h.title.moving(now) || h.levels(now) != h.painted)
    }

    pub(super) fn tick(&mut self, now: u64, px_per_dp: f32) -> bool {
        if let Some(bounce) = self.bounce {
            if bounce.at(now).1 {
                self.bounce = None;
            }
            return true;
        }
        let fading = self.header_needs_draw(now);
        let Some(fling) = self.fling else { return fading };
        let (y, done) = fling.at(now);
        let next = y.clamp(0.0, self.max_scroll);
        self.fling = (!done && y == next).then_some(fling);
        let changed = next.round() != std::mem::replace(&mut self.scroll, next).round();
        if next != y {
            let speed = fling.velocity(now);
            let period = if speed.abs() <= HARD_BOUNCE_DP * px_per_dp { 0.4 } else { 0.55 };
            self.bounce = Some(Spring::new(now, 0.0, speed, period));
        }
        changed || self.bounce.is_some() || fading
    }

    fn offset(&self, range: f32, now: u64) -> f32 {
        let over = self.bounce.map_or_else(|| motion::rubber(self.pull, range), |s| s.at(now).0);
        self.scroll + over
    }

    fn catch(&mut self, range: f32, now: u64) -> bool {
        let gliding = self.fling.take().is_some();
        let spring = self.bounce.take().map(|s| s.at(now)).filter(|&(_, done)| !done);
        if let Some((over, _)) = spring {
            self.pull = motion::unrubber(over, range);
        }
        gliding || spring.is_some()
    }
}

impl App {
    pub(super) fn set_menu_width(&mut self, width: u32) {
        if (self.menu.width == 0) != (width == 0)
            && let Some((Screen::Home, scroll, focus)) = self.stack.first_mut()
        {
            std::mem::swap(scroll, &mut self.menu.other_home_scroll);
            *focus = Focus::Unset;
        }
        self.menu.width = width;
    }

    pub(super) fn wide_menu(&self) -> bool {
        self.menu.width > 0
    }

    /// The first column of the detail pane, whatever page it shows.
    pub(super) fn pane_at(&self) -> Option<u32> {
        self.wide_menu().then_some(self.menu.width + SEPARATOR)
    }

    pub(super) fn keyboard_top(&self) -> u32 {
        self.h - self.frame.as_ref().and_then(Frame::wide_keyboard).map_or(0, |d| d.canvas.h)
    }

    pub(super) fn touch_wide_key(&mut self, ev: Touch, now: u64) -> Option<Vec<Effect>> {
        let key = match ev {
            Touch::Down(x, y) => {
                let top = self.keyboard_top() as f32;
                let dock = self.frame.as_ref().and_then(Frame::wide_keyboard).filter(|_| y >= top)?;
                let key = dock.hits.iter().find(|h| h.rect.contains(x, y - top)).map(|h| h.action);
                self.menu.cancel();
                self.let_row_go(now);
                self.drag = Drag::WideKey { start: (x, y), key };
                key
            }
            Touch::Move(x, y) => {
                let Drag::WideKey { start, ref mut key } = self.drag else { return None };
                if (x - start.0).hypot(y - start.1) > self.px_per_dp * 8.0 {
                    *key = None;
                }
                *key
            }
            Touch::Up => {
                let Drag::WideKey { key, .. } = self.drag else { return None };
                self.drag = Drag::Idle;
                self.state.held = None;
                // A page or lock state may have changed under the finger.
                let key = key.filter(|_| self.frame.as_ref().is_none_or(|fr| fr.keyboard.is_some_and(|k| k.enabled)));
                self.refresh();
                return Some(key.map_or_else(Vec::new, |action| self.act(action, now)));
            }
        };
        if std::mem::replace(&mut self.state.held, key) != key {
            self.refresh();
        }
        Some(Vec::new())
    }

    pub(super) fn drop_wide_key(&mut self) {
        if matches!(self.drag, Drag::WideKey { .. }) {
            self.drag = Drag::Idle;
            self.state.held = None;
        }
    }

    /// What the detail pane draws. About next to the menu is also the
    /// overview when no page has been opened from Home yet.
    pub fn shown_page(&self) -> Screen {
        if self.screen() == Screen::Home && self.pane_at().is_some() { Screen::About } else { self.screen() }
    }

    fn menu_selection(&self) -> Screen {
        if self.screen() == Screen::Reboot {
            return Screen::Reboot;
        }
        match self.stack[0].0 {
            Screen::Home => {}
            Screen::Installing => return Screen::Install,
            Screen::Erase | Screen::CantStart => return Screen::Wipe,
            root => return root,
        }
        match self.stack.get(1).map(|p| p.0) {
            None | Some(Screen::Specs | Screen::Logs) => Screen::About,
            Some(Screen::Search) => self.stack.get(2).map_or(Screen::Home, |p| p.0),
            Some(page) => page,
        }
    }

    pub(super) fn paint_menu(&mut self, out: &mut Canvas, f: &Fonts, info: &DeviceInfo, bottom: u32, now: u64) {
        let w = self.menu.width;
        let t = self.theme_at(w);
        if self.menu.frame.is_none() {
            let cx = Ctx { t: &t, f, info, state: &self.state, term: None, band: None };
            let frame = pages::home_menu(&cx, self.menu_selection());
            let fold = frame.fold.expect("the menu has a search bar");
            self.menu.art = Some(MenuArt {
                big: Blurs::new(widgets::big_title(&t, f, frame.title)),
                small: Blurs::new(widgets::small_title(&t, f, frame.title)),
                search: widgets::search_words(&t, f, fold.bar, fold.hint),
            });
            self.menu.frame = Some(frame);
        }
        let menu = self.menu.frame.as_ref().expect("the menu was drawn");
        self.menu.max_scroll = menu.canvas.h.saturating_sub(bottom) as f32;
        self.menu.scroll = self.menu.scroll.min(self.menu.max_scroll);
        let offset = self.menu.offset(bottom.max(1) as f32, now).round() as i32;
        let gap = (-offset).clamp(0, bottom as i32) as u32;
        let from = (offset.max(0) as u32).min(menu.canvas.h);
        let to = (from + bottom - gap).min(menu.canvas.h);
        let end = gap + to - from;
        if gap > 0 {
            out.fill_rect(0.0, 0.0, w as f32, gap as f32, t.pal.bg);
        }
        out.put_pane(&menu.canvas, 0, gap, from..to);
        if end < bottom {
            out.fill_rect(0.0, end as f32, w as f32, (bottom - end) as f32, t.pal.bg);
        }
        let scroll = self.menu.scroll;
        let art = self.menu.art.as_mut().expect("the menu's header was made with its page");
        if let Some(fold) = menu.fold {
            fold.draw(out, &t, &art.search, offset as f32);
        }
        draw_large_title(out, &t, menu, &mut art.big, scroll, offset as f32);
        let (title, mask) = scrolled_header(&t, menu, scroll);
        let header = self.menu.header.get_or_insert_with(|| MenuHeader::new(mask, title));
        header.mask.set(mask, now);
        header.title.set(title, now);
        header.painted = header.levels(now);
        let (mask, title) = header.painted;
        if mask > 0.0 {
            widgets::bar_mask(out, &t, &widgets::BAR_MASK, menu.bg, mask);
        }
        draw_faded(out, &mut art.small, 0.0, 0.0, t.pal.text, title, (1.0 - title) * t.m.dp(TITLE_BLUR_DP));
        out.fill_rect(w as f32, 0.0, SEPARATOR as f32, bottom as f32, t.pal.card);
    }

    /// A finger that lands on the menu stays there for the entire gesture.
    pub(super) fn touch_menu(&mut self, ev: Touch, now: u64) -> Option<Vec<Effect>> {
        let w = self.menu.width as f32;
        let ppd = self.px_per_dp;
        let range = self.keyboard_top().max(1) as f32;
        match ev {
            Touch::Down(x, y) if x < w + SEPARATOR as f32 => {
                let moving = self.menu.catch(range, now);
                self.menu.tracker.clear();
                self.menu.tracker.add(now, y);
                self.menu.drag = Some(if moving {
                    MenuDrag::Scroll { last: y }
                } else {
                    let t = self.theme_at(self.menu.width);
                    let top = if self.menu.header.as_ref().is_some_and(|h| h.mask.on) { widgets::bar_bottom(&t) } else { t.m.status_height };
                    let action = if x >= w || y < top.ceil() {
                        None
                    } else {
                        self.menu.frame.as_ref().and_then(|fr| {
                            let offset = self.menu.scroll.round();
                            fr.hit(x, y + offset).filter(|h| fr.shown(h.rect, offset).is_some_and(|r| r.contains(x, y)))
                        }).map(|h| h.action)
                    };
                    MenuDrag::Press { y, action }
                });
                self.drag = Drag::Idle;
            }
            Touch::Move(x, y) => {
                let drag = self.menu.drag.take()?;
                self.menu.tracker.add(now, y);
                let last = match drag {
                    MenuDrag::Press { y: start, action } if (y - start).abs() <= 8.0 * ppd => {
                        self.menu.drag = Some(MenuDrag::Press { y: start, action: if x < w && y < self.keyboard_top() as f32 { action } else { None } });
                        return Some(Vec::new());
                    }
                    MenuDrag::Press { y, .. } | MenuDrag::Scroll { last: y } => y,
                };
                let next = self.menu.scroll + self.menu.pull + last - y;
                self.menu.scroll = next.clamp(0.0, self.menu.max_scroll);
                self.menu.pull = next - self.menu.scroll;
                self.menu.drag = Some(MenuDrag::Scroll { last: y });
                self.dirty = true;
            }
            Touch::Up => match self.menu.drag.take()? {
                MenuDrag::Press { action: Some(Action::Open(page)), .. } => return Some(self.select_section(page, now)),
                MenuDrag::Scroll { .. } if self.menu.pull != 0.0 => {
                    let offset = motion::rubber(std::mem::take(&mut self.menu.pull), range);
                    self.menu.bounce = Some(Spring::new(now, offset, 0.0, 0.4));
                    self.dirty = true;
                }
                MenuDrag::Scroll { .. } => {
                    let speed = -self.menu.tracker.velocity(now);
                    if speed.abs() >= MIN_FLING_DP * ppd {
                        let speed = speed.clamp(-MAX_FLING_DP * ppd, MAX_FLING_DP * ppd);
                        self.menu.fling = Some(Fling::new(now, self.menu.scroll, speed, ppd * 160.0));
                    }
                }
                MenuDrag::Press { .. } => {}
            },
            _ => return None,
        }
        Some(Vec::new())
    }

    fn select_section(&mut self, page: Screen, now: u64) -> Vec<Effect> {
        // Startup unlock and system-requested modes only have their own
        // exit controls. For ordinary pages, use the same guards as Back.
        if self.stack[0].0 != Screen::Home || (self.shown_page() == page && self.stack.len() <= 2) {
            return Vec::new();
        }
        let mut fx = Vec::new();
        while self.stack.len() > 1 {
            let depth = self.stack.len();
            fx.extend(self.back_from(now, true));
            if self.stack.len() == depth {
                return fx;
            }
        }
        if page != Screen::About {
            fx.extend(self.act(Action::Open(page), now));
        }
        fx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::{battery_at, fonts, info, press};

    const W: u32 = 1916;
    const H: u32 = 2160;
    const PPD: f32 = 2.75;

    fn open() -> App {
        let mut app = App::new(Screen::Home, 1080, 2520, PPD);
        app.state.wlan = Some(Wlan::default());
        app.resize_panel(W, H, PPD, true, 0);
        app
    }

    fn tap(app: &mut App, f: &Fonts, (x, y): (f32, f32), now: u64) -> Vec<Effect> {
        let mut effects = app.touch(Touch::Down(x, y), f, now);
        effects.extend(app.touch(Touch::Up, f, now + 1));
        effects
    }

    fn menu_row(app: &App, page: Screen) -> (f32, f32) {
        let (x, y) = app.menu.frame.as_ref().unwrap().hits.iter().find(|h| h.action == Action::Open(page)).unwrap().rect.centre();
        (x, y - app.menu.scroll.round())
    }

    #[test]
    fn the_inner_navigation_uses_the_cover_status_mask() {
        let (f, mut i) = (fonts(), info());
        let mut app = open();
        // The menu needs enough real content for a card to reach the header.
        app.state.dual_boot = true;
        i.snapshot = SnapshotState::Snapshotted;
        app.set_status_bar(40, 16);
        let mut out = Canvas::new(W, H);
        app.compose(&mut out, &f, &i, "12:00", 0);

        let t = app.theme_at(app.menu.width);
        let x = t.m.margin + t.m.dp(12.0);
        let y = t.m.dp(58.0).round() as u32;
        let (_, source_y) = menu_row(&app, Screen::About);
        app.menu.scroll = source_y.round() - y as f32;
        app.compose(&mut out, &f, &i, "12:00", 1000);
        app.compose(&mut out, &f, &i, "12:00", 1200);
        assert!(app.menu.header.as_ref().unwrap().mask.on);
        let menu = &app.menu.frame.as_ref().unwrap().canvas;
        let pixel = |c: &Canvas, x: u32, y: u32| {
            let at = ((y * c.w + x) * 4) as usize;
            <[u8; 4]>::try_from(&c.data[at..at + 4]).unwrap()
        };
        let offset = app.menu.scroll.round() as u32;
        let cover_theme = app.theme_at(1080);
        let plain = widgets::bar_mask_height(&cover_theme).ceil() as u32;
        let status_end = cover_theme.m.status_height.ceil() as u32;
        let mut cover = Canvas::filled(1080, plain + 1, app.theme().pal.bg);
        cover.put_pane(menu, 0, 0, offset..offset + plain + 1);
        let behind = pixel(&cover, x as u32, y);
        widgets::bar_mask(&mut cover, &cover_theme, &widgets::BAR_MASK, app.theme().pal.bg, 1.0);
        assert_ne!(pixel(&cover, x as u32, y), behind, "the scrolled row needs the mask");
        for row in [status_end - 2, status_end + 1, plain - 8] {
            assert_eq!(pixel(&out, x as u32, row), pixel(&cover, x as u32, row), "the scroll mask matches the cover");
        }
        assert_eq!(pixel(&out, x as u32, plain), pixel(menu, x as u32, offset + plain), "the rows below the mask are unchanged");
        let separator = app.menu.width;
        assert_eq!(pixel(&out, separator, 0), pixel(&out, separator, status_end), "the divider also separates the two page backgrounds at the top");
        assert!(tap(&mut app, &f, (x, y as f32), 1300).is_empty());
        assert_eq!(app.screen(), Screen::Home, "menu rows behind the status bar cannot be tapped");
    }

    #[test]
    fn the_status_bar_follows_each_panes_cover_page() {
        const COVER: u32 = 1080;
        let (f, mut info) = (fonts(), info());
        info.battery = Some(battery_at(72, skyward_info::Charge::Charging));
        info.slot = Some(Slot::B);
        let mut outer = Canvas::new(COVER, 2520);
        let mut inner = Canvas::new(W, H);
        for dark in [false, true] {
            let mut app = App::new(Screen::Home, COVER, 2520, PPD);
            app.state.dark = dark;
            app.state.wlan = Some(Wlan::default());
            app.set_status_bar(40, 16);
            app.compose(&mut outer, &f, &info, "12:00", 1000);
            let m = app.theme().m;
            assert_eq!((m.status_height, m.status_cy, m.status_x, m.status_right, m.status_bolt_right), (110.0, 55.0, 44.0, 44.0, 44.0));
            let top = m.status_height as u32;
            let left = outer.crop(0, 0, COVER / 2, top);
            let home = app.frame(&f, &info);
            let search_top = home.fold.unwrap().bar.y;
            let card_top = home.content_top;
            let rows = |frame: &Frame| frame.hits.iter().map(|h| (h.action, h.rect.y, h.rect.h)).collect::<Vec<_>>();
            let home_rows = rows(home);
            let title_h = search_top.floor() as u32 - top;
            let left_title = outer.crop(0, top, COVER / 2, title_h);
            let plain = outer.crop(COVER / 2, 0, COVER / 2, 1);
            let mut about = App::new(Screen::About, COVER, 2520, PPD);
            about.state = app.state.clone();
            about.set_status_bar(40, 16);
            about.compose(&mut outer, &f, &info, "12:00", 1800);
            assert_ne!(outer.crop(COVER / 2, 0, COVER / 2, 1).data, plain.data, "About on the cover paints its original gradient up to the top");

            app.resize_panel(W, H, PPD, true, 1100);
            app.compose(&mut inner, &f, &info, "12:00", 1800);
            assert_eq!(app.shown_page(), Screen::About);
            assert!(inner.crop(0, 0, COVER / 2, top).data == left.data, "the menu keeps the cover Home bar beside About");
            let menu = app.menu.frame.as_ref().unwrap();
            assert_eq!((menu.fold.unwrap().bar.y, menu.content_top), (search_top, card_top), "Home positions its search and cards at the same height on both panels");
            assert_eq!(rows(menu), home_rows, "the split menu uses the same row heights and spacing");
            assert!(inner.crop(0, top, COVER / 2, title_h).data == left_title.data, "the logo starts at the same position");
            assert_ne!(inner.crop(W - COVER / 2, 0, COVER / 2, 1).data, plain.data, "the right pane also carries About's gradient to the top");

            app.open(Screen::Wlan);
            app.compose(&mut inner, &f, &info, "12:00", 2100);
            app.compose(&mut inner, &f, &info, "12:00", 2800);
            assert!(inner.crop(0, 0, COVER / 2, top).data == left.data, "changing the right page leaves the menu's status bar alone");
            let marks = inner.crop(W - COVER / 2, 0, COVER / 2, top);
            app.resize(COVER, 2520, PPD, 2900);
            app.compose(&mut outer, &f, &info, "12:00", 3000);
            let right = outer.crop(COVER / 2, 0, COVER / 2, top);
            let max_delta = marks.data.iter().zip(&right.data).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
            // Absolute x positions at different widths can round a rasterized edge differently.
            assert!(max_delta <= 1, "the right pane matches WLAN on the cover (maximum channel difference: {max_delta})");
        }
    }

    #[test]
    fn the_menu_title_blurs_then_sharpens_in_the_bar() {
        let (f, i) = (fonts(), info());
        let mut app = open();
        app.state.dark = true;
        let mut out = Canvas::new(W, H);
        app.compose(&mut out, &f, &i, "12:00", 1000);
        let fold = app.menu.frame.as_ref().unwrap().fold.unwrap();
        let peak = |c: &Canvas, y: u32, h| c.crop(50, y, 600, h).data.as_chunks::<4>().0.iter().map(|p| p[0]).max().unwrap();
        let sharp = peak(&out, 245, 120);
        app.menu.scroll = fold.span + PPD * TITLE_FADE_DP / 2.0;
        app.compose(&mut out, &f, &i, "12:00", 1100);
        let blurred = peak(&out, 245, 120);
        assert!(blurred > 0 && blurred < sharp / 2, "the large title softens during scroll");
        app.menu.scroll = fold.span + PPD * TITLE_FADE_DP + 5.0;
        app.compose(&mut out, &f, &i, "12:00", 1200);
        assert!(app.menu.moving(1250), "the small title keeps animating after the list stops");
        app.compose(&mut out, &f, &i, "12:00", 1800);
        assert!(peak(&out, 175, 70) > blurred, "the small title comes into focus in the bar");
        assert!(!app.menu.moving(1800));
    }

    #[test]
    fn the_menu_resists_and_springs_back_at_both_ends() {
        let (f, i) = (fonts(), info());
        let mut app = open();
        let mut out = Canvas::new(W, H);
        app.compose(&mut out, &f, &i, "12:00", 0);
        assert!(app.menu.max_scroll > 0.0);
        for (sign, edge) in [(-1.0, 0.0), (1.0, app.menu.max_scroll)] {
            app.menu.scroll = edge;
            app.compose(&mut out, &f, &i, "12:00", 1000);
            app.touch(Touch::Down(400.0, 1200.0), &f, 1100);
            app.touch(Touch::Move(400.0, 1200.0 - sign * 300.0), &f, 1160);
            let pulled = (app.menu.offset(H as f32, 1160) - edge) * sign;
            assert!(pulled > 0.0 && pulled < 300.0);
            assert_eq!(app.top().1, 0.0);
            app.touch(Touch::Up, &f, 1200);
            assert_eq!(app.deadline(1200), Some(0));
            app.compose(&mut out, &f, &i, "12:00", 1260);
            let returning = (app.menu.offset(H as f32, 1260) - edge) * sign;
            assert!(returning > 0.0 && returning < pulled);
            app.tick(2000);
            assert_eq!(app.menu.offset(H as f32, 2000), edge);
            app.compose(&mut out, &f, &i, "12:00", 2000);
            assert!(!app.menu.moving(2000));
        }
    }

    #[test]
    fn switching_sections_slides_from_the_detail_that_was_visible() {
        let (f, i) = (fonts(), info());
        let mut app = open();
        app.open(Screen::Settings);
        app.open(Screen::ScreenOff);
        let mut out = Canvas::new(W, H);
        app.compose(&mut out, &f, &i, "12:00", 1000);
        let pane = app.pane_at().unwrap();
        let departing = out.crop(pane, 140, W - pane, 800);
        let wlan = menu_row(&app, Screen::Wlan);
        tap(&mut app, &f, wlan, 1100);
        assert_eq!(app.screen(), Screen::Wlan);
        assert!(app.turn.is_some());
        app.compose(&mut out, &f, &i, "12:00", 1101);
        assert!(out.crop(pane, 140, W - pane, 800).data == departing.data, "the first frame still shows Screen-off time");
        app.compose(&mut out, &f, &i, "12:00", 1150);
        assert!(out.crop(pane, 140, W - pane, 800).data != departing.data, "the right pane moves between pages");
        app.compose(&mut out, &f, &i, "12:00", 1700);
        assert!(app.turn.is_none());
        assert_eq!(app.screen(), Screen::Wlan);
    }

    #[test]
    fn the_menu_and_detail_scroll_and_receive_taps_independently() {
        let (f, i) = (fonts(), info());
        let mut app = open();
        let mut out = Canvas::new(W, H);
        app.compose(&mut out, &f, &i, "12:00", 1000);
        let detail = app.pane_at().unwrap();
        assert_eq!(app.shown_page(), Screen::About);
        assert_eq!(app.frame(&f, &i).canvas.w + detail, W);
        let wlan = menu_row(&app, Screen::Wlan);
        tap(&mut app, &f, wlan, 1000);
        assert_eq!(app.screen(), Screen::Wlan);
        app.compose(&mut out, &f, &i, "12:00", 1700);
        let (x, y) = app.frame(&f, &i).hits.iter().find(|h| h.action == Action::Toggle(Switch::Wlan)).unwrap().rect.centre();
        assert_eq!(tap(&mut app, &f, (x + detail as f32, y), 1800), vec![Effect::Wlan(true)]);

        app.touch(Touch::Down(wlan.0, 1800.0), &f, 1900);
        // Moving over the divider must not send this swipe to the page.
        app.touch(Touch::Move(detail as f32 + 200.0, 1250.0), &f, 1950);
        assert!(app.touch(Touch::Up, &f, 2070).is_empty());
        assert!(app.menu.scroll > 0.0);
        assert_eq!(app.top().1, 0.0);
        assert_eq!(app.screen(), Screen::Wlan);

        // A separate right-pane swipe leaves the menu at the same row.
        let scroll = app.menu.scroll;
        app.compose(&mut out, &f, &i, "12:00", 2150);
        app.touch(Touch::Down(detail as f32 + 300.0, 1740.0), &f, 2200);
        app.touch(Touch::Move(detail as f32 + 300.0, 1240.0), &f, 2250);
        assert!(app.touch(Touch::Up, &f, 2370).is_empty());
        assert_eq!(app.menu.scroll, scroll);
    }

    #[test]
    fn changing_sections_reuses_back_guards_and_the_install_entry_context() {
        let mut app = open();
        app.open(Screen::Systems);
        app.state.systems_work = Some(SystemsWork::Switching(Who::Other));
        assert!(app.select_section(Screen::Install, 1000).is_empty());
        assert_eq!(app.screen(), Screen::Systems);
        assert!(app.toast.is_some());
        app.state.systems_work = None;
        app.act(Action::Open(Screen::Install), 2000);
        assert!(app.state.install_second_system);
        app.select_section(Screen::Install, 3000);
        assert_eq!(app.stack.iter().map(|p| p.0).collect::<Vec<_>>(), [Screen::Home, Screen::Install]);
        assert!(!app.state.install_second_system, "the regular Install row must not inherit the second-system entry");
    }

    #[test]
    fn folding_keeps_the_page_and_power_but_cancels_confirmation() {
        let f = fonts();
        let mut app = open();
        app.open(Screen::Wipe);
        app.drag = Drag::Confirm { which: Confirm::Wipe, track: Rect::new(10.0, 10.0, 800.0, 100.0), grab: 0.0 };
        app.state.confirm = 0.95;
        app.resize(1080, 2520, PPD, 1000);
        assert!(app.pane_at().is_none());
        assert_eq!(app.screen(), Screen::Wipe);
        assert!(app.touch(Touch::Up, &f, 1500).is_empty());
        assert_eq!(app.state.confirm, 0.0);
        app.sleep(2000);
        app.resize_panel(W, H, PPD, true, 10_000);
        assert_eq!(app.panel(10_000), None);
        assert_eq!(app.screen(), Screen::Wipe);
        press(&mut app, Key::Power, 11_000);
        assert!(app.display_on());
        assert!(app.pane_at().is_some());

        app.open(Screen::Installing);
        assert_eq!(app.theme().m.w + app.pane_at().unwrap(), W);
        app.back(12_000);
        assert!(app.pane_at().is_some());
        app.touchscreen_missing();
        assert_eq!(app.theme().m.w, W, "hardware keys can return to the ordinary menu when touch is missing");
    }

    #[test]
    fn the_full_width_keyboard_owns_its_touches_across_the_divider() {
        let (f, i) = (fonts(), info());
        let mut app = open();
        let mut out = Canvas::new(W, H);
        app.select_section(Screen::Search, 1000);
        app.compose(&mut out, &f, &i, "12:00", 1600);
        let key = |app: &App, ch| {
            let dock = app.frame.as_ref().unwrap().wide_keyboard().unwrap();
            let (x, y) = dock.hits.iter().find(|h| h.action == Action::Type(Cap::Char(ch))).unwrap().rect.centre();
            (x, y + app.keyboard_top() as f32)
        };
        let frame = app.frame.as_ref().unwrap();
        assert_eq!(frame.wide_keyboard().unwrap().canvas.w, W);
        assert_eq!(frame.height(&app.theme()), H as f32);
        assert!(!frame.hits.iter().any(|h| matches!(h.action, Action::Type(_))));
        assert!(focus::spots(&app.theme(), frame).iter().any(|s| s.group == Group::Dock && s.stop == Stop::Hit(Action::Type(Cap::Char('q')))));
        let (q, p) = (key(&app, 'q'), key(&app, 'p'));
        assert!(q.0 < app.menu.width as f32 && p.0 > app.pane_at().unwrap() as f32);
        let search = menu_row(&app, Screen::Search);
        app.touch(Touch::Down(q.0, q.1), &f, 1700);
        assert_eq!(app.state.key_held(), Some(Cap::Char('q')));
        let menu_before = app.menu.scroll;
        app.touch(Touch::Move(search.0, search.1), &f, 1730);
        assert!(app.touch(Touch::Up, &f, 1731).is_empty());
        assert_eq!(app.screen(), Screen::Search);
        assert_eq!(app.menu.scroll, menu_before);
        assert!(app.state.search.is_empty());
        app.compose(&mut out, &f, &i, "12:00", 1750);
        tap(&mut app, &f, q, 1800);
        app.compose(&mut out, &f, &i, "12:00", 1900);
        tap(&mut app, &f, p, 2000);
        assert_eq!(app.state.search, "qp");

        // Folding during a press must preserve the text and discard that
        // touch; the cover uses its own local key coordinates.
        app.compose(&mut out, &f, &i, "12:00", 2100);
        app.touch(Touch::Down(q.0, q.1), &f, 2200);
        app.resize(1080, 2520, PPD, 2300);
        assert!(app.touch(Touch::Up, &f, 2900).is_empty());
        assert_eq!(app.state.search, "qp");
        assert!(app.state.key_held().is_none());
        let compact = app.frame(&f, &i);
        assert_eq!(compact.canvas.w, 1080);
        assert!(compact.wide_keyboard().is_none());
        assert!(compact.hits.iter().any(|h| h.action == Action::Type(Cap::Char('q'))));
    }

    #[test]
    fn startup_pin_and_pattern_stay_inside_the_detail_pane() {
        let (f, i) = (fonts(), info());
        let mut app = App::new(Screen::Decrypt, W, H, PPD);
        app.state.data = Data::Locked { user: 0, keys: Keys::Digits, checking: false };
        app.resize_panel(W, H, PPD, true, 0);
        let mut out = Canvas::new(W, H);
        app.compose(&mut out, &f, &i, "12:00", 1000);
        let frame = app.frame.as_ref().unwrap();
        assert!(frame.wide_keyboard().is_none());
        assert_eq!(app.keyboard_top(), H);
        let (x, y) = frame.hits.iter().find(|h| h.action == Action::Type(Cap::Char('1'))).unwrap().rect.centre();
        let key = (x + app.pane_at().unwrap() as f32, y);
        let search = menu_row(&app, Screen::Search);
        assert!(search.1 < app.keyboard_top() as f32);
        assert!(tap(&mut app, &f, search, 1100).is_empty());
        assert_eq!(app.screen(), Screen::Decrypt);
        tap(&mut app, &f, key, 1200);
        assert_eq!(app.state.code, "1");

        app.state.data = Data::Locked { user: 0, keys: Keys::Digits, checking: true };
        app.refresh();
        app.compose(&mut out, &f, &i, "12:00", 1300);
        assert!(tap(&mut app, &f, key, 1400).is_empty());
        assert_eq!(app.state.code, "1", "keys cannot change a PIN being verified");

        app.state.data = Data::Locked { user: 0, keys: Keys::Pattern, checking: false };
        app.refresh();
        let frame = app.frame(&f, &i);
        assert!(frame.wide_keyboard().is_none());
        assert_eq!(frame.pattern.unwrap().0, frame.canvas.w as f32 / 2.0);
    }

    #[test]
    fn installation_stays_in_the_detail_pane_and_sidebar_respects_writes() {
        let (f, i) = (fonts(), info());
        let mut app = open();
        app.open(Screen::Install);
        app.open(Screen::Installing);
        app.state.source = Source::File(Place::Storage);
        let version = Some("OS3.0.9.0.VLRCNXM".to_string());
        app.state.install = Install::Confirming { version: version.clone(), wipe: false };
        let mut out = Canvas::new(W, H);
        app.compose(&mut out, &f, &i, "12:00", 1000);
        let frame = app.frame.as_ref().unwrap();
        assert_eq!(frame.canvas.w + app.pane_at().unwrap(), W);
        assert!(frame.hits.iter().any(|h| matches!(h.action, Action::Confirm(_)) && h.rect.x + h.rect.w <= frame.canvas.w as f32));
        assert!(frame.wide_keyboard().is_none());

        app.state.install = Install::Updating { version, stage: Some(Stage::Writing { done: 600, total: 1200 }), overwriting: false };
        app.refresh();
        app.compose(&mut out, &f, &i, "12:00", 1200);
        let settings = menu_row(&app, Screen::Settings);
        assert!(tap(&mut app, &f, settings, 1300).is_empty());
        assert_eq!(app.screen(), Screen::Installing);
        assert!(matches!(app.state.install, Install::Updating { .. }));
        assert_eq!(app.menu_selection(), Screen::Install);
        assert!(app.toast.is_some());
        assert!(app.turn.is_none(), "a blocked switch does not animate to another page");
    }

    #[test]
    fn the_charging_screen_and_about_use_their_own_viewports() {
        let (f, mut i) = (fonts(), info());
        i.battery = Some(battery_at(75, skyward_info::Charge::Charging));
        let mut app = open();
        let mut out = Canvas::new(W, H);
        app.compose(&mut out, &f, &i, "12:00", 1000);
        app.plugged(1100, true);
        // Both surfaces paint on the same frame while the overlay comes up.
        app.compose(&mut out, &f, &i, "12:00", 1130);
        app.compose(&mut out, &f, &i, "12:00", 1500);
        app.end_charging();
        app.compose(&mut out, &f, &i, "12:00", 1600);
    }
}
