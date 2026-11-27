//! Страница подкаста, книги и сказки (`NonMusicContent` оригинала, `8390-…js`, модуль 98390):
//! вкладки «О подкасте» и «N выпусков» («О книге», «О сказке» и «Оглавление»), на первой —
//! описание и сведения, у подкаста ещё «Последние выпуски». Под названием выпуска — дата
//! (`PodcastMeta`, `4872-…js`, модуль 48215).

use super::*;
use crate::api::AlbumAbout;
use crate::landing::MONTHS;
use chrono::{Datelike, NaiveDate};

/// Подписи вкладок: `podcast.tab-about` и `podcast.tab-tracks` («{N} выпуск…» по числу всех
/// выпусков), у книги `non-music.audiobook-tab-about` и `…-tab-tracks`, у сказки
/// `non-music.fairytale-tab-about`.
fn tabs(kind: &str, tracks: usize) -> [String; 2] {
    let tr = crate::i18n::tr;
    match kind {
        "audiobook" => [tr("О книге"), tr("Оглавление")],
        "fairy-tale" => [tr("О сказке"), tr("Оглавление")],
        _ => {
            let n = tracks as u64;
            [tr("О подкасте"), format!("{n} {}", plural(n, "выпуск", "выпуска", "выпусков"))]
        }
    }
}

/// «Продолжительность»: `time.hours` и `time.minutes-left` через пробел, часы и минуты — с
/// округлением вниз (функция `J` модуля), каждая часть только если больше нуля.
fn duration_text(sec: u64) -> String {
    let (h, m) = (sec / 3600, sec / 60 % 60);
    let mut parts = Vec::new();
    if h > 0 {
        parts.push(format!("{h} {}", plural(h, "час", "часа", "часов")));
    }
    if m > 0 {
        parts.push(format!("{m} {}", plural(m, "минута", "минуты", "минут")));
    }
    parts.join(" ")
}

/// Строки сведений вкладки «О …» в порядке оригинала: чтецы (`album.artists`, «Чтец» /
/// «Чтецы»), «Продолжительность» при `durationSec`, издатели (у подкаста «Издатель» /
/// «Издатели», у книги «Лейбл» / «Лейблы» — `page.album-publisher-title` и
/// `page.album-label-title`), «Возрастное ограничение: 18+» при `contentWarning: explicit`.
/// У строки — названия и id лейблов: с id название ведёт на страницу лейбла.
fn about_lines(a: &Album, about: &AlbumAbout) -> Vec<(String, Vec<String>, Vec<String>)> {
    let tr = crate::i18n::tr;
    let one = |text: String| (vec![text], vec![String::new()]);
    let mut out = Vec::new();
    if !a.artist_list.is_empty() {
        let title = if a.artist_list.len() > 1 { "Чтецы" } else { "Чтец" };
        let (items, ids) = one(a.artist_list.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", "));
        out.push((format!("{}:", tr(title)), items, ids));
    }
    if about.duration_sec > 0 {
        let (items, ids) = one(duration_text(about.duration_sec));
        out.push((format!("{}:", tr("Продолжительность")), items, ids));
    }
    if !about.labels.is_empty() {
        let many = about.labels.len() > 1;
        let title = match (a.kind == "podcast", many) {
            (true, false) => "Издатель",
            (true, true) => "Издатели",
            (false, false) => "Лейбл",
            (false, true) => "Лейблы",
        };
        let items = about.labels.iter().map(|(_, name)| name.clone()).collect();
        let ids = about.labels.iter().map(|(id, _)| id.map(|id| id.to_string()).unwrap_or_default()).collect();
        out.push((format!("{}:", tr(title)), items, ids));
    }
    if about.explicit {
        let (items, ids) = one("18+".into());
        out.push((format!("{}:", tr("Возрастное ограничение")), items, ids));
    }
    out
}

/// Дата выпуска (модуль 32947): «Сегодня» и «Вчера» — подсвечены (`soonDate`), в этом году —
/// «24 ноября», в другом — «24 ноября 2025 г.», как `formatDate` с `year: numeric`.
fn episode_date(day: NaiveDate, today: NaiveDate) -> (String, bool) {
    if day == today {
        return (crate::i18n::tr("Сегодня"), true);
    }
    if today.pred_opt() == Some(day) {
        return (crate::i18n::tr("Вчера"), true);
    }
    let text = format!("{} {}", day.day(), MONTHS[day.month0() as usize]);
    if day.year() == today.year() {
        (text, false)
    } else {
        (format!("{text} {} г.", day.year()), false)
    }
}

/// Даты под названиями выпусков на странице подкаста. Подкаст на своей странице не пишется
/// (`withPodcastName` не задан), исполнители тоже: вторая строка — только дата.
pub(super) fn set_episode_dates(l: &ListData, view: &[usize], items: &mut [TrackItem]) {
    if !l.album.as_ref().is_some_and(|a| a.kind == "podcast") {
        return;
    }
    let today = chrono::Local::now().date_naive();
    for (item, &i) in items.iter_mut().zip(view) {
        if let Some(day) = l.tracks[i].pub_date {
            let (text, soon) = episode_date(day, today);
            item.date = text.into();
            item.soon = soon;
        }
    }
}

impl Ctx {
    /// Вкладки и «О …» немузыкального альбома. У музыки вкладок нет, страница прежняя.
    pub(super) fn show_album_about(&self, l: &ListData) {
        let ui = self.ui();
        let app = ui.global::<App>();
        let strings = |v: Vec<String>| ModelRc::from(Rc::new(VecModel::from(v.into_iter().map(SharedString::from).collect::<Vec<_>>())));
        let about = l.album.as_ref().and_then(|a| Some((a, a.about.as_deref()?)));
        app.set_list_podcast(about.is_some_and(|(a, _)| a.kind == "podcast"));
        let Some((a, about)) = about else {
            app.set_list_tabs(ModelRc::default());
            app.set_list_about(ModelRc::default());
            return;
        };
        app.set_list_tabs(strings(tabs(&a.kind, l.tracks.len()).into()));
        app.set_list_desc(about.description.clone().into());
        let lines: Vec<AboutLine> = about_lines(a, about)
            .into_iter()
            .map(|(title, items, ids)| AboutLine { title: title.into(), items: strings(items), ids: strings(ids) })
            .collect();
        app.set_list_about(ModelRc::from(Rc::new(VecModel::from(lines))));
        // Издатель — в сведениях на вкладке «О …»; строки «Лейбл:» под списком, как у музыкального
        // альбома, у оригинала здесь нет.
        app.set_list_label(SharedString::new());
        app.set_list_label_id(SharedString::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_titles() {
        assert_eq!(tabs("podcast", 366), ["О подкасте".to_string(), "366 выпусков".into()]);
        assert_eq!(tabs("podcast", 21)[1], "21 выпуск");
        assert_eq!(tabs("podcast", 3)[1], "3 выпуска");
        assert_eq!(tabs("audiobook", 5), ["О книге".to_string(), "Оглавление".into()]);
        assert_eq!(tabs("fairy-tale", 1), ["О сказке".to_string(), "Оглавление".into()]);
    }

    #[test]
    fn durations() {
        assert_eq!(duration_text(2 * 3600 + 5 * 60 + 59), "2 часа 5 минут");
        assert_eq!(duration_text(3600), "1 час");
        assert_eq!(duration_text(11 * 3600 + 21 * 60), "11 часов 21 минута");
        assert_eq!(duration_text(30), "");
    }

    #[test]
    fn dates() {
        let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert_eq!(episode_date(today, today), ("Сегодня".into(), true));
        assert_eq!(episode_date(d(2025, 12, 31), today), ("Вчера".into(), true));
        assert_eq!(episode_date(d(2025, 11, 24), today), ("24 ноября 2025 г.".into(), false));
        let today = d(2026, 11, 27);
        assert_eq!(episode_date(d(2026, 11, 24), today), ("24 ноября".into(), false));
    }
}
