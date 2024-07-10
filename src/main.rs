use anyhow::anyhow;
use chrono::{Utc, Months, Datelike, NaiveDate, TimeZone};
use futures::StreamExt;
use scraper::Node;
use std::hash::{Hash, Hasher};

const NUM_MONTHS: u32 = 14; // 2 months of "going back", plus one year

fn url_for(year: usize, month: usize) -> String {
    let user = std::env::var("REMOTEUSER").expect("REMOTEUSER must be configured");
    let pass = std::env::var("REMOTEPASS").expect("REMOTEPASS must be configured");
    format!("http://{user}:{pass}@brionac.s17.xrea.com/schedule/homepage/homepage/calendar/{year}/{year}{month:02}.html")
}

#[derive(Debug, Hash)]
struct Time {
    hours: usize,
    minutes: usize,
}

#[derive(Debug, Hash)]
enum Event {
    Timed {
        day: usize,
        from: Time,
        to: Time,
        text: String,
    },
    FullDay {
        day: usize,
        text: String,
    }
}

impl Event {
    fn append(&mut self, append: &str) {
        match self {
            Event::Timed { text, .. } => {
                text.push(' ');
                text.push_str(append);
            }
            Event::FullDay { text, .. } => {
                text.push(' ');
                text.push_str(append);
            }
        }
    }

    fn as_ics(&self, year: usize, month: usize) -> String {
        let mut hasher = std::hash::DefaultHasher::new();
        self.hash(&mut hasher);
        let hash = hasher.finish();
        let (start, end, text) = match self {
            Event::FullDay { day, text } => {
                let day = format!("DATE:{year:04}{month:02}{day:02}");
                (format!("DTSTART;VALUE={day}"), format!("DTEND;VALUE={day}"), text)
            }
            Event::Timed { day, from, to, text } => {
                let year = year.try_into().unwrap();
                let month = month.try_into().unwrap();
                let day = (*day).try_into().unwrap();
                let from_hours = from.hours.try_into().unwrap();
                let to_hours = to.hours.try_into().unwrap();
                let from_mins = from.minutes.try_into().unwrap();
                let to_mins = to.minutes.try_into().unwrap();
                let from = chrono_tz::Asia::Tokyo.with_ymd_and_hms(year, month, day, from_hours, from_mins, 0).unwrap().with_timezone(&Utc).format("DTSTART:%Y%m%dT%H%M%SZ");
                let to = chrono_tz::Asia::Tokyo.with_ymd_and_hms(year, month, day, to_hours, to_mins, 0).unwrap().with_timezone(&Utc).format("DTEND:%Y%m%dT%H%M%SZ");
                (format!("{from}"), format!("{to}"), text)
            }
        };
        #[cfg(not(test))]
        let now = Utc::now().format("%Y%m%dT%H%M%SZ");
        #[cfg(test)]
        let now = "20000101T000000Z";

        let url = url_for(year, month);
        format!(
            "BEGIN:VEVENT\n\
             UID:{hash}@shinbukan-ics\n\
             DTSTAMP:{now}\n\
             {start}\n\
             {end}\n\
             SUMMARY:{text}\n\
             URL:{url}\n\
             END:VEVENT\n"
        )
    }
}

#[derive(Debug)]
struct MonthResult {
    year: usize,
    month: usize,
    events: Vec<Event>,
    errors: Vec<anyhow::Error>,
}

impl MonthResult {
    fn new(year: usize, month: usize) -> MonthResult {
        MonthResult {
            year,
            month,
            events: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn event(&mut self, day: usize, mut from: Time, mut to: Time, text: &str) {
        // Hours are set in 12 am/pm format, but without the am/pm indication
        if from.hours < 8 {
            from.hours += 12;
        }
        if to.hours < 8 {
            to.hours += 12;
        }
        self.events.push(Event::Timed { day, from, to, text: text.to_owned() })
    }

    fn full_day_event(&mut self, day: usize, text: &str) {
        self.events.push(Event::FullDay { day, text: text.to_owned() })
    }

    fn append_to_last_event(&mut self, text: &str) {
        self.events.last_mut().unwrap().append(text);
    }

    fn error(&mut self, err: anyhow::Error) {
        self.errors.push(err);
    }

    fn days_in_month(&self) -> usize {
        let first_day = NaiveDate::from_ymd_opt(self.year.try_into().unwrap(), self.month.try_into().unwrap(), 1).unwrap();
        let next_month = first_day + Months::new(1);
        let interval = next_month - first_day;
        interval.num_days().try_into().unwrap()
    }

    fn events_as_ics(&self) -> String {
        let mut res = String::new();
        for e in &self.events {
            res.push_str(&e.as_ics(self.year, self.month));
        }
        res
    }

    fn errors(&self) -> &[anyhow::Error] {
        &self.errors
    }
}

async fn fetch_calendar_for(year: usize, month: usize) -> anyhow::Result<String> {
    let url = url_for(year, month);
    tracing::debug!(%url, "fetching calendar page");
    let resp = reqwest::get(url).await?;
    let bytes = resp.bytes().await?;
    let text = encoding_rs::EUC_JP.decode(&bytes).0.into_owned();
    Ok(text)
}

fn parse_calendar(res: &mut MonthResult, cal: &str) {
    let doc = scraper::Html::parse_document(cal);
    let selector = scraper::Selector::parse(r#"table[summary="日程"] td"#).unwrap();
    let mut parsed_days = vec![false; res.days_in_month()];
    for element in doc.select(&selector) {
        if let Some(day) = parse_cell(&mut *res, &element) {
            if !parsed_days[day - 1] {
                parsed_days[day - 1] = true;
            } else {
                res.error(anyhow!("Parsed day {day} twice"));
            }
        }
    }
    for day in 0..parsed_days.len() {
        if !parsed_days[day] {
            res.error(anyhow!("Did not parse day {}", day + 1));
        }
    }
}

fn get_day_number(elt: &Node) -> Option<usize> {
    let Node::Text(txt) = elt else {
        return None;
    };
    Some(txt.trim().parse().unwrap())
}

fn parse_time(time: &str) -> Time {
    match time.split_once(':') {
        None => Time { hours: time.parse().unwrap(), minutes: 0 },
        Some((hours, minutes)) => Time { hours: hours.parse().unwrap(), minutes: minutes.parse().unwrap() },
    }
}

// Returns the number of the parsed day, if applicable
fn parse_cell(res: &mut MonthResult, cell: &scraper::ElementRef<'_>) -> Option<usize> {
    let mut children = cell.children();
    let Some(day_num_elt) = children.next() else {
        return None;
    };
    let Some(day_num) = get_day_number(day_num_elt.value()) else {
        return None;
    };
    while let Some(c) = children.next() {
        match c.value() {
            Node::Element(elt) => match elt.name() {
                "br" => continue,
                "font" if elt.attr("size") == Some("-1") => continue,
                "font" if elt.attr("color") == Some("red") => {
                    for n in c.descendants() {
                        if let Node::Text(txt) = n.value() {
                            res.append_to_last_event(txt);
                        }
                    }
                }
                _ => res.error(anyhow!("Encountered unexpected element while parsing day {day_num}: {elt:?}")),
            }
            Node::Text(txt) => {
                let txt = txt.trim();
                if txt.is_empty() {
                    continue;
                }
                match txt.split_once(' ') {
                    None => res.full_day_event(day_num, txt),
                    Some((time, rem)) => match time.split_once(&['-', '~']) {
                        None => res.full_day_event(day_num, txt),
                        Some((from, to)) => res.event(day_num, parse_time(from), parse_time(to), rem),
                    }
                }
            }
            _ => res.error(anyhow!("Encountered unexpected node while parsing day {day_num}: {:?}", c.value())),
        }
    }
    Some(day_num)
}

async fn handle_month(year: usize, month: usize) -> MonthResult {
    let mut result = MonthResult::new(year, month);
    let cal = match fetch_calendar_for(year, month).await {
        Ok(cal) => cal,
        Err(err) => {
            result.error(err);
            return result;
        }
    };
    parse_calendar(&mut result, &cal);
    result
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let today = Utc::now().naive_utc().date();
    let first_date = today - Months::new(2);

    // Parse the calendar
    let results = futures::stream::iter(0..NUM_MONTHS)
        .map(|add_months| {
            let for_date = first_date + Months::new(add_months);
            let for_year = for_date.year().try_into().unwrap();
            let for_month = for_date.month().try_into().unwrap();
            handle_month(for_year, for_month)
        })
        .buffered(16)
        .collect::<Vec<MonthResult>>()
        .await;

    // Generate the ICS file
    println!("BEGIN:VCALENDAR");
    println!("VERSION:2.0");
    println!("PRODID:-//Shinbukan-ICS//Shinbukan-ICS//");
    println!("NAME:Shinbukan");
    println!("X-WR-CALNAME:Shinbukan");
    let mut had_errors = false;
    for res in results {
        print!("{}", res.events_as_ics());
        if !res.errors().is_empty() {
            for e in res.errors() {
                eprintln!("---");
                eprintln!("Error occurred while processing the online calendar!");
                eprintln!("{e:?}");
                eprintln!("---");
            }
            had_errors = true;
        }
    }
    println!("END:VCALENDAR");

    if !had_errors {
        Ok(())
    } else {
        Err(anyhow!("Errors occurred while processing the input"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_fixtures() {
        insta::glob!("fixtures/*.html", |path| {
            // Retrieve year/month from filename
            let filename = path.file_name().unwrap().to_str().unwrap();
            let yearmonth = filename.split_once('.').unwrap().0;
            let (year, month) = yearmonth.split_once('-').unwrap();
            let year = year.parse().unwrap();
            let month = month.parse().unwrap();
            let mut result = MonthResult::new(year, month);

            // Read file and parse calendar
            let input = std::fs::read_to_string(path).unwrap();
            parse_calendar(&mut result, &input);

            // Assert the snapshot
            insta::assert_debug_snapshot!(result);

            // Generate the relevant ICS file
            insta::assert_snapshot!(result.events_as_ics());
        })
    }
}
