use crate::models::{
    CustomAppGroup, HistoryBar, HistoryReport, HistoryRouteFilter, HistoryRow,
    HistorySubjectFilter, RouteClass,
};
use anyhow::{Context, Result};
use chrono::{Datelike, Local, TimeZone, Timelike};
use rusqlite::{Connection, params};
use std::collections::BTreeMap;
use std::path::Path;

pub struct HistoryStore {
    conn: Connection,
}

struct FilteredSample {
    app: String,
    route: String,
    route_class: RouteClass,
    rule_id: String,
    rule_label: String,
    download: u64,
    upload: u64,
}

#[derive(Debug, Clone)]
pub struct FlowRecord {
    pub app: String,
    pub route: String,
    pub route_class: RouteClass,
    pub rule_id: Option<String>,
    pub rule_label: Option<String>,
    pub download: u64,
    pub upload: u64,
}

impl HistoryStore {
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn =
            Connection::open(path).context("failed to open the traffic history database")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS minute_totals (
                minute INTEGER PRIMARY KEY,
                download INTEGER NOT NULL DEFAULT 0,
                upload INTEGER NOT NULL DEFAULT 0,
                gap INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS minute_apps (
                minute INTEGER NOT NULL,
                app TEXT NOT NULL,
                route TEXT NOT NULL,
                route_class TEXT NOT NULL DEFAULT 'unknown',
                rule_id TEXT NOT NULL DEFAULT '',
                rule_label TEXT NOT NULL DEFAULT '',
                download INTEGER NOT NULL DEFAULT 0,
                upload INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (minute, app, route, route_class, rule_id)
            );
            CREATE INDEX IF NOT EXISTS idx_minute_apps_minute ON minute_apps(minute);
            CREATE TABLE IF NOT EXISTS gap_ranges (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at INTEGER NOT NULL,
                ended_at INTEGER
            );",
        )?;
        migrate_route_class(&mut conn)?;
        migrate_rule_columns(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn record_totals(&mut self, at: i64, download: u64, upload: u64) -> Result<()> {
        if download == 0 && upload == 0 {
            return Ok(());
        }
        let minute = minute_start(at);
        self.conn.execute(
            "INSERT INTO minute_totals(minute, download, upload, gap) VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(minute) DO UPDATE SET
                download = download + excluded.download,
                upload = upload + excluded.upload",
            params![minute, to_i64(download)?, to_i64(upload)?],
        )?;
        Ok(())
    }

    pub fn record_apps(
        &mut self,
        at: i64,
        flows: &[(String, String, RouteClass, u64, u64)],
    ) -> Result<()> {
        let flows = flows
            .iter()
            .map(|(app, route, route_class, download, upload)| FlowRecord {
                app: app.clone(),
                route: route.clone(),
                route_class: *route_class,
                rule_id: None,
                rule_label: None,
                download: *download,
                upload: *upload,
            })
            .collect::<Vec<_>>();
        self.record_flows(at, &flows)
    }

    pub fn record_flows(&mut self, at: i64, flows: &[FlowRecord]) -> Result<()> {
        if flows.is_empty() {
            return Ok(());
        }
        let minute = minute_start(at);
        let tx = self.conn.transaction()?;
        for flow in flows {
            if flow.download == 0 && flow.upload == 0 {
                continue;
            }
            tx.execute(
                "INSERT INTO minute_apps(minute, app, route, route_class, rule_id, rule_label, download, upload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(minute, app, route, route_class, rule_id) DO UPDATE SET
                    rule_label = excluded.rule_label,
                    download = download + excluded.download,
                    upload = upload + excluded.upload",
                params![
                    minute,
                    flow.app,
                    flow.route,
                    route_class_name(flow.route_class),
                    flow.rule_id.as_deref().unwrap_or(""),
                    flow.rule_label.as_deref().unwrap_or(""),
                    to_i64(flow.download)?,
                    to_i64(flow.upload)?,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn mark_gap(&mut self, at: i64) -> Result<()> {
        let minute = minute_start(at);
        self.conn.execute(
            "INSERT INTO minute_totals(minute, download, upload, gap) VALUES (?1, 0, 0, 1)
             ON CONFLICT(minute) DO UPDATE SET gap = 1",
            params![minute],
        )?;
        Ok(())
    }

    pub fn prune(&mut self, now: i64) -> Result<()> {
        let cutoff = now.saturating_sub(30 * 24 * 60 * 60);
        self.conn.execute(
            "DELETE FROM minute_totals WHERE minute < ?1",
            params![cutoff],
        )?;
        self.conn
            .execute("DELETE FROM minute_apps WHERE minute < ?1", params![cutoff])?;
        Ok(())
    }

    pub fn clear(&mut self) -> Result<()> {
        self.conn.execute("DELETE FROM minute_totals", [])?;
        self.conn.execute("DELETE FROM minute_apps", [])?;
        self.conn.execute("DELETE FROM gap_ranges", [])?;
        Ok(())
    }

    pub fn report(
        &self,
        range: &str,
        breakdown: &str,
        groups: &[CustomAppGroup],
        now: i64,
    ) -> Result<HistoryReport> {
        self.report_with_filters(range, breakdown, groups, now, None, None)
    }

    pub fn report_with_filters(
        &self,
        range: &str,
        breakdown: &str,
        groups: &[CustomAppGroup],
        now: i64,
        route_filter: Option<HistoryRouteFilter>,
        subject_filter: Option<&HistorySubjectFilter>,
    ) -> Result<HistoryReport> {
        let (from, bar_mode) = range_bounds(range, now)?;
        let available_apps = self.available_apps(from, now)?;
        let (total_download, total_upload, gap_minutes, bars, rows) =
            if route_filter.is_none() && subject_filter.is_none() {
                let (total_download, total_upload, gap_minutes, bars) =
                    self.totals_and_bars(from, now, bar_mode)?;
                let rows =
                    self.build_rows(from, now, breakdown, groups, total_download, total_upload)?;
                (total_download, total_upload, gap_minutes, bars, rows)
            } else {
                self.filtered_report_parts(
                    from,
                    now,
                    bar_mode,
                    breakdown,
                    groups,
                    route_filter,
                    subject_filter,
                )?
            };
        Ok(HistoryReport {
            from,
            to: now,
            total_download,
            total_upload,
            total: total_download.saturating_add(total_upload),
            gap_minutes,
            bars,
            rows,
            breakdown: breakdown.to_owned(),
            available_apps,
        })
    }

    fn totals_and_bars(
        &self,
        from: i64,
        to: i64,
        bar_mode: BarMode,
    ) -> Result<(u64, u64, u64, Vec<HistoryBar>)> {
        let mut stmt = self.conn.prepare(
            "SELECT minute, download, upload, gap FROM minute_totals WHERE minute >= ?1 AND minute <= ?2 ORDER BY minute",
        )?;
        let rows = stmt.query_map(params![from, to], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        let mut totals_down = 0u64;
        let mut totals_up = 0u64;
        let mut gaps = 0u64;
        let mut totals_by_minute = BTreeMap::new();
        let mut buckets: BTreeMap<i64, (u64, u64, u64, u64, bool)> = BTreeMap::new();
        for start in bar_bucket_starts(from, to, bar_mode) {
            buckets.insert(start, (0, 0, 0, 0, false));
        }
        for row in rows {
            let (minute, down, up, gap) = row?;
            let down = u64::try_from(down.max(0)).unwrap_or(0);
            let up = u64::try_from(up.max(0)).unwrap_or(0);
            totals_down = totals_down.saturating_add(down);
            totals_up = totals_up.saturating_add(up);
            totals_by_minute.insert(minute, (down, up));
            if gap != 0 {
                gaps = gaps.saturating_add(1);
            }
            let bucket = bar_bucket_start(minute, bar_mode);
            let entry = buckets.entry(bucket).or_insert((0, 0, 0, 0, false));
            entry.0 = entry.0.saturating_add(down);
            entry.1 = entry.1.saturating_add(up);
            entry.4 |= gap != 0;
        }

        let mut route_stmt = self.conn.prepare(
            "SELECT minute, route_class, sum(download), sum(upload) FROM minute_apps
             WHERE minute >= ?1 AND minute <= ?2 AND route_class IN ('proxy', 'direct')
             GROUP BY minute, route_class ORDER BY minute, route_class",
        )?;
        let route_rows = route_stmt.query_map(params![from, to], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        let mut routes_by_minute: BTreeMap<i64, (u64, u64, u64, u64)> = BTreeMap::new();
        for row in route_rows {
            let (minute, route_class, download, upload) = row?;
            let routes = routes_by_minute.entry(minute).or_default();
            match route_class.as_str() {
                "proxy" => {
                    routes.0 = u64::try_from(download.max(0)).unwrap_or(0);
                    routes.1 = u64::try_from(upload.max(0)).unwrap_or(0);
                }
                "direct" => {
                    routes.2 = u64::try_from(download.max(0)).unwrap_or(0);
                    routes.3 = u64::try_from(upload.max(0)).unwrap_or(0);
                }
                _ => {}
            }
        }
        for (minute, (mut proxy_down, mut proxy_up, mut direct_down, mut direct_up)) in
            routes_by_minute
        {
            let (limit_down, limit_up) = totals_by_minute.get(&minute).copied().unwrap_or_default();
            let estimated_down = proxy_down as u128 + direct_down as u128;
            let estimated_up = proxy_up as u128 + direct_up as u128;
            if estimated_down > limit_down as u128 {
                proxy_down = scale_to_total(proxy_down, limit_down, estimated_down);
                direct_down = scale_to_total(direct_down, limit_down, estimated_down);
            }
            if estimated_up > limit_up as u128 {
                proxy_up = scale_to_total(proxy_up, limit_up, estimated_up);
                direct_up = scale_to_total(direct_up, limit_up, estimated_up);
            }
            let bucket = buckets
                .entry(bar_bucket_start(minute, bar_mode))
                .or_insert((0, 0, 0, 0, false));
            bucket.2 = bucket.2.saturating_add(proxy_down.saturating_add(proxy_up));
            bucket.3 = bucket
                .3
                .saturating_add(direct_down.saturating_add(direct_up));
        }
        let bars = buckets
            .into_iter()
            .map(
                |(start, (download, upload, proxy_total, direct_total, gap))| HistoryBar {
                    start,
                    label: bar_label(start, bar_mode),
                    download,
                    upload,
                    proxy_total,
                    direct_total,
                    gap,
                },
            )
            .collect();
        Ok((totals_down, totals_up, gaps, bars))
    }

    fn available_apps(&self, from: i64, to: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT app FROM minute_apps
             WHERE minute >= ?1 AND minute <= ?2 AND trim(app) <> ''
             ORDER BY app COLLATE NOCASE, app",
        )?;
        let rows = stmt.query_map(params![from, to], |row| row.get::<_, String>(0))?;
        let mut apps = Vec::new();
        for row in rows {
            let app = row?;
            if !apps
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(&app))
            {
                apps.push(app);
            }
        }
        Ok(apps)
    }

    fn filtered_report_parts(
        &self,
        from: i64,
        to: i64,
        bar_mode: BarMode,
        breakdown: &str,
        groups: &[CustomAppGroup],
        route_filter: Option<HistoryRouteFilter>,
        subject_filter: Option<&HistorySubjectFilter>,
    ) -> Result<(u64, u64, u64, Vec<HistoryBar>, Vec<HistoryRow>)> {
        let mut total_stmt = self.conn.prepare(
            "SELECT minute, download, upload, gap FROM minute_totals
             WHERE minute >= ?1 AND minute <= ?2 ORDER BY minute",
        )?;
        let total_rows = total_stmt.query_map(params![from, to], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        let mut totals_by_minute = BTreeMap::new();
        let mut gap_minutes = 0u64;
        let mut buckets: BTreeMap<i64, (u64, u64, u64, u64, bool)> =
            bar_bucket_starts(from, to, bar_mode)
                .into_iter()
                .map(|start| (start, (0, 0, 0, 0, false)))
                .collect();
        for row in total_rows {
            let (minute, download, upload, gap) = row?;
            totals_by_minute.insert(
                minute,
                (
                    u64::try_from(download.max(0)).unwrap_or(0),
                    u64::try_from(upload.max(0)).unwrap_or(0),
                ),
            );
            if gap != 0 {
                gap_minutes = gap_minutes.saturating_add(1);
                buckets
                    .entry(bar_bucket_start(minute, bar_mode))
                    .or_default()
                    .4 = true;
            }
        }

        let mut sample_stmt = self.conn.prepare(
            "SELECT minute, app, route, route_class, rule_id, rule_label, download, upload FROM minute_apps
             WHERE minute >= ?1 AND minute <= ?2 ORDER BY minute, app, route, route_class, rule_id",
        )?;
        let sample_rows = sample_stmt.query_map(params![from, to], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })?;
        let mut samples_by_minute: BTreeMap<i64, Vec<FilteredSample>> = BTreeMap::new();
        for row in sample_rows {
            let (minute, app, route, class, rule_id, rule_label, download, upload) = row?;
            let route_class = route_class_from_db(&class);
            let route_matches = match route_filter {
                None => true,
                Some(HistoryRouteFilter::Proxy) => route_class == RouteClass::Proxy,
                Some(HistoryRouteFilter::Direct) => route_class == RouteClass::Direct,
            };
            let subject_matches = match subject_filter {
                None => true,
                Some(HistorySubjectFilter::App { name }) => app.eq_ignore_ascii_case(name),
                Some(HistorySubjectFilter::Group { id }) => groups
                    .iter()
                    .find(|group| group.id == *id)
                    .is_some_and(|group| {
                        group
                            .apps
                            .iter()
                            .any(|candidate| candidate.eq_ignore_ascii_case(&app))
                    }),
            };
            if route_matches && subject_matches {
                samples_by_minute
                    .entry(minute)
                    .or_default()
                    .push(FilteredSample {
                        app,
                        route,
                        route_class,
                        rule_id,
                        rule_label,
                        download: u64::try_from(download.max(0)).unwrap_or(0),
                        upload: u64::try_from(upload.max(0)).unwrap_or(0),
                    });
            }
        }

        let mut total_download = 0u64;
        let mut total_upload = 0u64;
        let mut aggregate: BTreeMap<
            (String, RouteClass),
            (u64, u64, Option<String>, Option<String>),
        > = BTreeMap::new();
        for (minute, samples) in samples_by_minute {
            let (limit_download, limit_upload) =
                totals_by_minute.get(&minute).copied().unwrap_or_default();
            let estimated_download: u128 =
                samples.iter().map(|sample| sample.download as u128).sum();
            let estimated_upload: u128 = samples.iter().map(|sample| sample.upload as u128).sum();
            let bucket = buckets
                .entry(bar_bucket_start(minute, bar_mode))
                .or_default();
            for sample in samples {
                let download = if estimated_download > limit_download as u128 {
                    scale_to_total(sample.download, limit_download, estimated_download)
                } else {
                    sample.download
                };
                let upload = if estimated_upload > limit_upload as u128 {
                    scale_to_total(sample.upload, limit_upload, estimated_upload)
                } else {
                    sample.upload
                };
                total_download = total_download.saturating_add(download);
                total_upload = total_upload.saturating_add(upload);
                bucket.0 = bucket.0.saturating_add(download);
                bucket.1 = bucket.1.saturating_add(upload);
                match sample.route_class {
                    RouteClass::Proxy => {
                        bucket.2 = bucket.2.saturating_add(download.saturating_add(upload));
                    }
                    RouteClass::Direct => {
                        bucket.3 = bucket.3.saturating_add(download.saturating_add(upload));
                    }
                    RouteClass::Blocked | RouteClass::Unknown => {}
                }

                let (label, app, route) = row_identity(
                    breakdown,
                    &sample.app,
                    &sample.route,
                    &sample.rule_id,
                    &sample.rule_label,
                    groups,
                    subject_filter,
                );
                let entry = aggregate
                    .entry((label, sample.route_class))
                    .or_insert((0, 0, app, route));
                entry.0 = entry.0.saturating_add(download);
                entry.1 = entry.1.saturating_add(upload);
            }
        }

        let mut rows = aggregate
            .into_iter()
            .map(
                |((label, route_class), (download, upload, app, route))| HistoryRow {
                    label,
                    app,
                    route,
                    route_class,
                    download,
                    upload,
                    total: download.saturating_add(upload),
                },
            )
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            right
                .total
                .cmp(&left.total)
                .then(left.label.cmp(&right.label))
        });
        let bars = buckets
            .into_iter()
            .map(
                |(start, (download, upload, proxy_total, direct_total, gap))| HistoryBar {
                    start,
                    label: bar_label(start, bar_mode),
                    download,
                    upload,
                    proxy_total,
                    direct_total,
                    gap,
                },
            )
            .collect();
        Ok((total_download, total_upload, gap_minutes, bars, rows))
    }

    fn build_rows(
        &self,
        from: i64,
        to: i64,
        breakdown: &str,
        groups: &[CustomAppGroup],
        total_download: u64,
        total_upload: u64,
    ) -> Result<Vec<HistoryRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT app, route, route_class, rule_id, max(rule_label), sum(download), sum(upload) FROM minute_apps
             WHERE minute >= ?1 AND minute <= ?2 GROUP BY app, route, route_class, rule_id",
        )?;
        let raw = stmt.query_map(params![from, to], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;
        let mut aggregate: BTreeMap<
            (String, RouteClass),
            (u64, u64, Option<String>, Option<String>),
        > = BTreeMap::new();
        for row in raw {
            let (app, route, class, rule_id, rule_label, down, up) = row?;
            let route_class = route_class_from_db(&class);
            let down = u64::try_from(down.max(0)).unwrap_or(0);
            let up = u64::try_from(up.max(0)).unwrap_or(0);
            let (label, app_field, route_field) = match breakdown {
                "rule" => {
                    let label = if rule_id.is_empty() {
                        "Unattributed".to_owned()
                    } else if rule_label.is_empty() {
                        "Rule".to_owned()
                    } else {
                        rule_label
                    };
                    (label, None, None)
                }
                "customGroup" => {
                    let group = groups.iter().find(|group| {
                        group
                            .apps
                            .iter()
                            .any(|candidate| candidate.eq_ignore_ascii_case(&app))
                    });
                    let label = group
                        .map(|group| group.name.clone())
                        .unwrap_or_else(|| app.clone());
                    let app_field = group.is_none().then_some(app.clone());
                    (label, app_field, None)
                }
                "route" => (route.clone(), None, Some(route.clone())),
                _ => (app.clone(), Some(app.clone()), None),
            };
            let entry =
                aggregate
                    .entry((label, route_class))
                    .or_insert((0, 0, app_field, route_field));
            entry.0 = entry.0.saturating_add(down);
            entry.1 = entry.1.saturating_add(up);
        }
        let estimated_down: u128 = aggregate.values().map(|value| value.0 as u128).sum();
        let estimated_up: u128 = aggregate.values().map(|value| value.1 as u128).sum();
        let mut rows = Vec::new();
        for ((label, route_class), (down, up, app, route)) in aggregate {
            let (down, up) =
                if estimated_down > total_download as u128 || estimated_up > total_upload as u128 {
                    (
                        scale_to_total(down, total_download, estimated_down),
                        scale_to_total(up, total_upload, estimated_up),
                    )
                } else {
                    (down, up)
                };
            rows.push(HistoryRow {
                label,
                app,
                route,
                route_class,
                download: down,
                upload: up,
                total: down.saturating_add(up),
            });
        }
        let attributed_down: u128 = rows.iter().map(|row| row.download as u128).sum();
        let attributed_up: u128 = rows.iter().map(|row| row.upload as u128).sum();
        let residual_down = (total_download as u128).saturating_sub(attributed_down);
        let residual_up = (total_upload as u128).saturating_sub(attributed_up);
        if residual_down > 0 || residual_up > 0 {
            let down = u64::try_from(residual_down).unwrap_or(u64::MAX);
            let up = u64::try_from(residual_up).unwrap_or(u64::MAX);
            if let Some(row) = rows.iter_mut().find(|row| {
                row.label == "Unattributed"
                    && row.app.is_none()
                    && row.route_class == RouteClass::Unknown
                    && (breakdown != "route" || row.route.as_deref() == Some("Unattributed"))
            }) {
                row.download = row.download.saturating_add(down);
                row.upload = row.upload.saturating_add(up);
                row.total = row.download.saturating_add(row.upload);
            } else {
                rows.push(HistoryRow {
                    label: "Unattributed".to_owned(),
                    app: None,
                    route: if breakdown == "route" {
                        Some("Unattributed".to_owned())
                    } else {
                        None
                    },
                    route_class: RouteClass::Unknown,
                    download: down,
                    upload: up,
                    total: down.saturating_add(up),
                });
            }
        }
        rows.sort_by(|left, right| {
            right
                .total
                .cmp(&left.total)
                .then(left.label.cmp(&right.label))
        });
        Ok(rows)
    }
}

fn migrate_route_class(conn: &mut Connection) -> Result<()> {
    let has_route_class = {
        let mut statement = conn.prepare("PRAGMA table_info(minute_apps)")?;
        let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
        columns
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|column| column == "route_class")
    };
    if has_route_class {
        return Ok(());
    }

    let tx = conn.transaction()?;
    tx.execute_batch(
        "DROP INDEX IF EXISTS idx_minute_apps_minute;
         ALTER TABLE minute_apps RENAME TO minute_apps_legacy;
         CREATE TABLE minute_apps (
             minute INTEGER NOT NULL,
             app TEXT NOT NULL,
             route TEXT NOT NULL,
             route_class TEXT NOT NULL DEFAULT 'unknown',
             download INTEGER NOT NULL DEFAULT 0,
             upload INTEGER NOT NULL DEFAULT 0,
             PRIMARY KEY (minute, app, route, route_class)
         );
         INSERT INTO minute_apps(minute, app, route, route_class, download, upload)
         SELECT minute, app, route,
             CASE lower(trim(route))
                 WHEN 'direct' THEN 'direct'
                 WHEN 'blocked' THEN 'blocked'
                 WHEN 'reject' THEN 'blocked'
                 WHEN '' THEN 'unknown'
                 WHEN 'unknown' THEN 'unknown'
                 WHEN 'unattributed' THEN 'unknown'
                 ELSE 'proxy'
             END,
             download, upload
         FROM minute_apps_legacy;
         DROP TABLE minute_apps_legacy;
         CREATE INDEX idx_minute_apps_minute ON minute_apps(minute);",
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_rule_columns(conn: &mut Connection) -> Result<()> {
    let has_rule_id = {
        let mut statement = conn.prepare("PRAGMA table_info(minute_apps)")?;
        let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
        columns
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|column| column == "rule_id")
    };
    if has_rule_id {
        return Ok(());
    }

    let tx = conn.transaction()?;
    tx.execute_batch(
        "DROP INDEX IF EXISTS idx_minute_apps_minute;
         ALTER TABLE minute_apps RENAME TO minute_apps_legacy;
         CREATE TABLE minute_apps (
             minute INTEGER NOT NULL,
             app TEXT NOT NULL,
             route TEXT NOT NULL,
             route_class TEXT NOT NULL DEFAULT 'unknown',
             rule_id TEXT NOT NULL DEFAULT '',
             rule_label TEXT NOT NULL DEFAULT '',
             download INTEGER NOT NULL DEFAULT 0,
             upload INTEGER NOT NULL DEFAULT 0,
             PRIMARY KEY (minute, app, route, route_class, rule_id)
         );
         INSERT INTO minute_apps(minute, app, route, route_class, rule_id, rule_label, download, upload)
         SELECT minute, app, route, route_class, '', '', download, upload
         FROM minute_apps_legacy;
         DROP TABLE minute_apps_legacy;
         CREATE INDEX idx_minute_apps_minute ON minute_apps(minute);",
    )?;
    tx.commit()?;
    Ok(())
}

fn route_class_name(class: RouteClass) -> &'static str {
    match class {
        RouteClass::Proxy => "proxy",
        RouteClass::Direct => "direct",
        RouteClass::Blocked => "blocked",
        RouteClass::Unknown => "unknown",
    }
}

fn route_class_from_db(class: &str) -> RouteClass {
    match class {
        "proxy" => RouteClass::Proxy,
        "direct" => RouteClass::Direct,
        "blocked" => RouteClass::Blocked,
        _ => RouteClass::Unknown,
    }
}

fn row_identity(
    breakdown: &str,
    app: &str,
    route: &str,
    rule_id: &str,
    rule_label: &str,
    groups: &[CustomAppGroup],
    subject_filter: Option<&HistorySubjectFilter>,
) -> (String, Option<String>, Option<String>) {
    match breakdown {
        "rule" => {
            let label = if rule_id.is_empty() {
                "Unattributed".to_owned()
            } else if rule_label.is_empty() {
                "Rule".to_owned()
            } else {
                rule_label.to_owned()
            };
            (label, None, None)
        }
        "customGroup" => {
            let selected_group_id = match subject_filter {
                Some(HistorySubjectFilter::Group { id }) => Some(id.as_str()),
                _ => None,
            };
            let group = selected_group_id
                .and_then(|id| groups.iter().find(|group| group.id == id))
                .filter(|group| {
                    group
                        .apps
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(app))
                })
                .or_else(|| {
                    groups.iter().find(|group| {
                        group
                            .apps
                            .iter()
                            .any(|candidate| candidate.eq_ignore_ascii_case(app))
                    })
                });
            let label = group
                .map(|group| group.name.clone())
                .unwrap_or_else(|| app.to_owned());
            let app_field = group.is_none().then(|| app.to_owned());
            (label, app_field, None)
        }
        "route" => (route.to_owned(), None, Some(route.to_owned())),
        _ => (app.to_owned(), Some(app.to_owned()), None),
    }
}

fn scale_to_total(value: u64, total: u64, estimated: u128) -> u64 {
    if estimated == 0 {
        return 0;
    }
    let scaled = (value as u128).saturating_mul(total as u128) / estimated;
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

fn to_i64(value: u64) -> Result<i64> {
    i64::try_from(value).context("traffic counter overflowed the local history")
}

fn minute_start(timestamp: i64) -> i64 {
    timestamp - timestamp.rem_euclid(60)
}

#[derive(Clone, Copy)]
enum BarMode {
    Hour,
    Day,
}

fn range_bounds(range: &str, now: i64) -> Result<(i64, BarMode)> {
    let local = Local
        .timestamp_opt(now, 0)
        .single()
        .unwrap_or_else(Local::now);
    let today = Local
        .with_ymd_and_hms(local.year(), local.month(), local.day(), 0, 0, 0)
        .single()
        .unwrap_or(local)
        .timestamp();
    match range {
        "today" => Ok((today, BarMode::Hour)),
        "7d" => Ok((today - 6 * 86_400, BarMode::Day)),
        "30d" => Ok((today - 29 * 86_400, BarMode::Day)),
        _ => anyhow::bail!("unsupported history range"),
    }
}

fn bar_bucket_start(timestamp: i64, mode: BarMode) -> i64 {
    let local = Local
        .timestamp_opt(timestamp, 0)
        .single()
        .unwrap_or_else(Local::now);
    match mode {
        BarMode::Hour => Local
            .with_ymd_and_hms(local.year(), local.month(), local.day(), local.hour(), 0, 0)
            .single()
            .unwrap_or(local)
            .timestamp(),
        BarMode::Day => Local
            .with_ymd_and_hms(local.year(), local.month(), local.day(), 0, 0, 0)
            .single()
            .unwrap_or(local)
            .timestamp(),
    }
}

fn bar_bucket_starts(from: i64, to: i64, mode: BarMode) -> Vec<i64> {
    let start = bar_bucket_start(from, mode);
    let end = bar_bucket_start(to, mode);
    let mut result = Vec::new();
    let step = match mode {
        BarMode::Hour => 3_600,
        BarMode::Day => 86_400,
    };
    let mut cursor = start;
    while cursor <= end {
        result.push(cursor);
        cursor = cursor.saturating_add(step);
    }
    result
}

fn bar_label(start: i64, mode: BarMode) -> String {
    let local = Local
        .timestamp_opt(start, 0)
        .single()
        .unwrap_or_else(Local::now);
    match mode {
        BarMode::Hour => local.format("%H:%M").to_string(),
        BarMode::Day => local.format("%b %-d").to_string(),
    }
}

pub fn retention_cutoff(now: i64) -> i64 {
    now.saturating_sub(30 * 24 * 60 * 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> HistoryStore {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE minute_totals (
                minute INTEGER PRIMARY KEY,
                download INTEGER NOT NULL DEFAULT 0,
                upload INTEGER NOT NULL DEFAULT 0,
                gap INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE minute_apps (
                minute INTEGER NOT NULL,
                app TEXT NOT NULL,
                route TEXT NOT NULL,
                route_class TEXT NOT NULL DEFAULT 'unknown',
                rule_id TEXT NOT NULL DEFAULT '',
                rule_label TEXT NOT NULL DEFAULT '',
                download INTEGER NOT NULL DEFAULT 0,
                upload INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (minute, app, route, route_class, rule_id)
            );
            CREATE TABLE gap_ranges (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at INTEGER NOT NULL,
                ended_at INTEGER
            );",
        )
        .unwrap();
        HistoryStore { conn }
    }

    #[test]
    fn totals_are_authoritative_and_residual_is_unattributed() {
        let mut history = memory();
        let now = Local::now().timestamp();
        history.record_totals(now, 1_000, 400).unwrap();
        history
            .record_apps(
                now,
                &[(
                    "ChatGPT.exe".into(),
                    "Singapore".into(),
                    RouteClass::Proxy,
                    600,
                    100,
                )],
            )
            .unwrap();
        let report = history.report("today", "app", &[], now).unwrap();
        assert_eq!(report.total_download, 1_000);
        assert_eq!(report.total_upload, 400);
        let rows_total: u64 = report.rows.iter().map(|row| row.total).sum();
        assert_eq!(rows_total, report.total);
        assert!(report.rows.iter().any(|row| row.label == "Unattributed"));
    }

    #[test]
    fn overflow_is_scaled_to_authoritative_total() {
        let mut history = memory();
        let now = Local::now().timestamp();
        history.record_totals(now, 100, 50).unwrap();
        history
            .record_apps(
                now,
                &[
                    ("A".into(), "Direct".into(), RouteClass::Direct, 500, 100),
                    ("B".into(), "Node".into(), RouteClass::Proxy, 500, 100),
                ],
            )
            .unwrap();
        let report = history.report("today", "app", &[], now).unwrap();
        assert_eq!(report.rows.iter().map(|row| row.download).sum::<u64>(), 100);
        assert_eq!(report.rows.iter().map(|row| row.upload).sum::<u64>(), 50);
    }

    #[test]
    fn gap_and_custom_group_are_retroactive() {
        let mut history = memory();
        let now = Local::now().timestamp();
        history.record_totals(now, 10, 10).unwrap();
        history
            .record_apps(
                now,
                &[(
                    "codex.exe".into(),
                    "Direct".into(),
                    RouteClass::Direct,
                    10,
                    10,
                )],
            )
            .unwrap();
        history.mark_gap(now).unwrap();
        let groups = vec![CustomAppGroup {
            id: "g".into(),
            name: "AI".into(),
            apps: vec!["codex.exe".into()],
        }];
        let report = history
            .report("today", "customGroup", &groups, now)
            .unwrap();
        assert!(report.rows.iter().any(|row| row.label == "AI"));
        assert_eq!(report.gap_minutes, 1);
        assert!(report.bars.iter().any(|bar| bar.gap));
    }

    #[test]
    fn retention_removes_old_minutes() {
        let mut history = memory();
        let now = Local::now().timestamp();
        history.record_totals(now - 40 * 86_400, 10, 10).unwrap();
        history.record_totals(now, 20, 20).unwrap();
        history.prune(now).unwrap();
        let count: i64 = history
            .conn
            .query_row("SELECT count(*) FROM minute_totals", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn zero_and_duplicate_deltas_do_not_double_count() {
        let mut history = memory();
        let now = Local::now().timestamp();
        history.record_totals(now, 0, 0).unwrap();
        history.record_totals(now, 7, 3).unwrap();
        history.record_totals(now, 0, 0).unwrap();
        let report = history.report("today", "app", &[], now).unwrap();
        assert_eq!(report.total_download, 7);
        assert_eq!(report.total_upload, 3);
    }

    #[test]
    fn mixed_route_classes_remain_distinct_for_apps_groups_and_route_details() {
        let mut history = memory();
        let now = Local::now().timestamp();
        history.record_totals(now, 1_000, 500).unwrap();
        history
            .record_apps(
                now,
                &[
                    (
                        "ChatGPT.exe".into(),
                        "Falemon / Hong Kong".into(),
                        RouteClass::Proxy,
                        100,
                        40,
                    ),
                    (
                        "ChatGPT.exe".into(),
                        "Direct".into(),
                        RouteClass::Direct,
                        200,
                        100,
                    ),
                    (
                        "ChatGPT.exe".into(),
                        "Blocked".into(),
                        RouteClass::Blocked,
                        50,
                        20,
                    ),
                    (
                        "ChatGPT.exe".into(),
                        "Unknown chain".into(),
                        RouteClass::Unknown,
                        25,
                        10,
                    ),
                    (
                        "Other.exe".into(),
                        "Falemon / Hong Kong".into(),
                        RouteClass::Proxy,
                        100,
                        40,
                    ),
                ],
            )
            .unwrap();
        let groups = vec![CustomAppGroup {
            id: "ai".into(),
            name: "AI tools".into(),
            apps: vec!["ChatGPT.exe".into()],
        }];

        let app = history.report("today", "app", &groups, now).unwrap();
        assert_eq!(app.rows.iter().map(|row| row.download).sum::<u64>(), 1_000);
        assert_eq!(app.rows.iter().map(|row| row.upload).sum::<u64>(), 500);
        for class in [
            RouteClass::Proxy,
            RouteClass::Direct,
            RouteClass::Blocked,
            RouteClass::Unknown,
        ] {
            assert!(
                app.rows
                    .iter()
                    .any(|row| row.label == "ChatGPT.exe" && row.route_class == class)
            );
        }
        assert!(
            app.rows
                .iter()
                .any(|row| row.label == "Unattributed" && row.route_class == RouteClass::Unknown)
        );
        let chart_period = app
            .bars
            .iter()
            .find(|bar| bar.start == bar_bucket_start(minute_start(now), BarMode::Hour))
            .unwrap();
        assert_eq!(chart_period.proxy_total, 280);
        assert_eq!(chart_period.direct_total, 300);
        assert!(chart_period.proxy_total + chart_period.direct_total < app.total);

        let grouped = history
            .report("today", "customGroup", &groups, now)
            .unwrap();
        assert_eq!(
            grouped.rows.iter().map(|row| row.download).sum::<u64>(),
            1_000
        );
        assert_eq!(grouped.rows.iter().map(|row| row.upload).sum::<u64>(), 500);
        assert!(
            grouped
                .rows
                .iter()
                .any(|row| row.label == "AI tools" && row.route_class == RouteClass::Proxy)
        );
        assert!(
            grouped
                .rows
                .iter()
                .any(|row| row.label == "AI tools" && row.route_class == RouteClass::Direct)
        );

        let routes = history.report("today", "route", &groups, now).unwrap();
        assert_eq!(
            routes.rows.iter().map(|row| row.download).sum::<u64>(),
            1_000
        );
        assert_eq!(routes.rows.iter().map(|row| row.upload).sum::<u64>(), 500);
        assert!(
            routes
                .rows
                .iter()
                .any(|row| row.label == "Falemon / Hong Kong"
                    && row.route_class == RouteClass::Proxy)
        );
        assert!(
            routes
                .rows
                .iter()
                .any(|row| row.label == "Direct" && row.route_class == RouteClass::Direct)
        );
        assert!(
            routes
                .rows
                .iter()
                .any(|row| row.label == "Blocked" && row.route_class == RouteClass::Blocked)
        );
        assert!(
            routes
                .rows
                .iter()
                .any(|row| row.label == "Unknown chain" && row.route_class == RouteClass::Unknown)
        );
        assert!(
            routes
                .rows
                .iter()
                .any(|row| row.label == "Unattributed" && row.route_class == RouteClass::Unknown)
        );
    }

    #[test]
    fn route_and_group_filters_compose_and_keep_gaps() {
        let mut history = memory();
        let now = Local::now().timestamp();
        let previous_minute = minute_start(now - 60);
        let current_minute = minute_start(now);
        history.record_totals(previous_minute, 100, 50).unwrap();
        history.mark_gap(previous_minute).unwrap();
        history
            .record_apps(
                previous_minute,
                &[
                    (
                        "ChatGPT.exe".into(),
                        "Proxy node".into(),
                        RouteClass::Proxy,
                        30,
                        10,
                    ),
                    (
                        "ChatGPT.exe".into(),
                        "Direct".into(),
                        RouteClass::Direct,
                        20,
                        10,
                    ),
                    (
                        "codex.exe".into(),
                        "Proxy node".into(),
                        RouteClass::Proxy,
                        50,
                        30,
                    ),
                ],
            )
            .unwrap();
        history.record_totals(current_minute, 100, 50).unwrap();
        history
            .record_apps(
                current_minute,
                &[
                    (
                        "ChatGPT.exe".into(),
                        "Direct".into(),
                        RouteClass::Direct,
                        180,
                        80,
                    ),
                    (
                        "steam.exe".into(),
                        "Direct".into(),
                        RouteClass::Direct,
                        20,
                        10,
                    ),
                    (
                        "codex.exe".into(),
                        "Proxy node".into(),
                        RouteClass::Proxy,
                        10,
                        5,
                    ),
                ],
            )
            .unwrap();

        let groups = vec![
            CustomAppGroup {
                id: "first-match".into(),
                name: "First match".into(),
                apps: vec!["ChatGPT.exe".into()],
            },
            CustomAppGroup {
                id: "ai".into(),
                name: "AI tools".into(),
                apps: vec!["chatgpt.exe".into()],
            },
            CustomAppGroup {
                id: "development".into(),
                name: "Development".into(),
                apps: vec!["CODEX.EXE".into()],
            },
        ];
        let ai_filter = HistorySubjectFilter::Group { id: "ai".into() };
        let filtered = history
            .report_with_filters(
                "7d",
                "customGroup",
                &groups,
                now,
                Some(HistoryRouteFilter::Direct),
                Some(&ai_filter),
            )
            .unwrap();

        assert_eq!((filtered.total_download, filtered.total_upload), (120, 60));
        assert_eq!(
            filtered.rows.iter().map(|row| row.download).sum::<u64>(),
            filtered.total_download
        );
        assert_eq!(
            filtered.rows.iter().map(|row| row.upload).sum::<u64>(),
            filtered.total_upload
        );
        assert!(
            filtered
                .rows
                .iter()
                .all(|row| { row.label == "AI tools" && row.route_class == RouteClass::Direct })
        );
        assert!(!filtered.rows.iter().any(|row| row.label == "Unattributed"));
        assert_eq!(
            filtered.bars.iter().map(|bar| bar.download).sum::<u64>(),
            filtered.total_download
        );
        assert_eq!(
            filtered.bars.iter().map(|bar| bar.upload).sum::<u64>(),
            filtered.total_upload
        );
        assert_eq!(filtered.gap_minutes, 1);
        assert!(filtered.bars.iter().any(|bar| {
            bar.gap && bar.start == bar_bucket_start(previous_minute, BarMode::Day)
        }));
        assert_eq!(
            filtered.available_apps,
            vec!["ChatGPT.exe", "codex.exe", "steam.exe"]
        );
        assert_eq!(
            filtered
                .bars
                .iter()
                .map(|bar| bar.direct_total)
                .sum::<u64>(),
            filtered.total
        );
        assert_eq!(
            filtered.bars.iter().map(|bar| bar.proxy_total).sum::<u64>(),
            0,
            "a Direct filter must keep the Proxy chart lane empty"
        );

        let proxy_only = history
            .report_with_filters(
                "7d",
                "customGroup",
                &groups,
                now,
                Some(HistoryRouteFilter::Proxy),
                Some(&ai_filter),
            )
            .unwrap();
        assert_eq!(
            proxy_only
                .bars
                .iter()
                .map(|bar| bar.proxy_total)
                .sum::<u64>(),
            proxy_only.total
        );
        assert_eq!(
            proxy_only
                .bars
                .iter()
                .map(|bar| bar.direct_total)
                .sum::<u64>(),
            0,
            "a Proxy filter must keep the Direct chart lane empty"
        );

        let no_match = history
            .report_with_filters(
                "7d",
                "app",
                &groups,
                now,
                Some(HistoryRouteFilter::Direct),
                Some(&HistorySubjectFilter::Group {
                    id: "development".into(),
                }),
            )
            .unwrap();
        assert_eq!(no_match.total, 0);
        assert!(no_match.rows.is_empty());
        assert_eq!(no_match.gap_minutes, 1);
        assert!(no_match.bars.iter().any(|bar| bar.gap));
        assert!(
            no_match
                .bars
                .iter()
                .all(|bar| bar.proxy_total == 0 && bar.direct_total == 0)
        );
        assert_eq!(no_match.available_apps, filtered.available_apps);
    }

    #[test]
    fn route_chart_values_exclude_other_classes_and_stay_within_minute_totals() {
        let mut history = memory();
        let now = Local::now().timestamp();
        history.record_totals(now, 100, 50).unwrap();
        history
            .record_apps(
                now,
                &[
                    (
                        "A.exe".into(),
                        "Proxy node".into(),
                        RouteClass::Proxy,
                        20,
                        10,
                    ),
                    ("B.exe".into(), "Direct".into(), RouteClass::Direct, 180, 80),
                    (
                        "C.exe".into(),
                        "Blocked".into(),
                        RouteClass::Blocked,
                        50,
                        25,
                    ),
                    (
                        "D.exe".into(),
                        "Unknown".into(),
                        RouteClass::Unknown,
                        60,
                        30,
                    ),
                ],
            )
            .unwrap();

        let all = history.report("today", "app", &[], now).unwrap();
        let period = all
            .bars
            .iter()
            .find(|bar| bar.start == bar_bucket_start(minute_start(now), BarMode::Hour))
            .unwrap();
        assert_eq!(period.proxy_total, 15);
        assert_eq!(period.direct_total, 134);
        assert!(period.proxy_total + period.direct_total <= all.total);

        let direct = history
            .report_with_filters(
                "today",
                "app",
                &[],
                now,
                Some(HistoryRouteFilter::Direct),
                None,
            )
            .unwrap();
        assert_eq!(direct.total, 150);
        assert_eq!(
            direct.bars.iter().map(|bar| bar.direct_total).sum::<u64>(),
            150
        );
        assert_eq!(
            direct.bars.iter().map(|bar| bar.proxy_total).sum::<u64>(),
            0
        );

        let proxy = history
            .report_with_filters(
                "today",
                "app",
                &[],
                now,
                Some(HistoryRouteFilter::Proxy),
                None,
            )
            .unwrap();
        assert_eq!(proxy.total, 30);
        assert_eq!(
            proxy.bars.iter().map(|bar| bar.proxy_total).sum::<u64>(),
            30
        );
        assert_eq!(
            proxy.bars.iter().map(|bar| bar.direct_total).sum::<u64>(),
            0
        );
    }

    #[test]
    fn app_filter_is_case_insensitive_and_uses_only_attributed_samples() {
        let mut history = memory();
        let now = Local::now().timestamp();
        history.record_totals(now, 100, 50).unwrap();
        history
            .record_apps(
                now,
                &[
                    (
                        "Steam.exe".into(),
                        "Direct".into(),
                        RouteClass::Direct,
                        20,
                        10,
                    ),
                    (
                        "ChatGPT.exe".into(),
                        "Proxy node".into(),
                        RouteClass::Proxy,
                        30,
                        20,
                    ),
                ],
            )
            .unwrap();
        let app_filter = HistorySubjectFilter::App {
            name: "STEAM.EXE".into(),
        };
        let filtered = history
            .report_with_filters(
                "today",
                "route",
                &[],
                now,
                Some(HistoryRouteFilter::Direct),
                Some(&app_filter),
            )
            .unwrap();
        assert_eq!((filtered.total_download, filtered.total_upload), (20, 10));
        assert_eq!(filtered.rows.len(), 1);
        assert_eq!(filtered.rows[0].label, "Direct");
        assert_eq!(filtered.rows[0].route_class, RouteClass::Direct);
        assert_eq!(
            filtered.bars.iter().map(|bar| bar.download).sum::<u64>(),
            20
        );
    }

    #[test]
    fn rule_breakdown_separates_sibling_rules_and_catch_all() {
        let mut history = memory();
        let now = Local::now().timestamp();
        history.record_totals(now, 1_000, 400).unwrap();
        history
            .record_flows(
                now,
                &[
                    FlowRecord {
                        app: "ChatGPT.exe".into(),
                        route: "Falemon / Singapore".into(),
                        route_class: RouteClass::Proxy,
                        rule_id: Some("ruleset-1:google".into()),
                        rule_label: Some("Google".into()),
                        download: 300,
                        upload: 100,
                    },
                    FlowRecord {
                        app: "ChatGPT.exe".into(),
                        route: "Falemon / Singapore".into(),
                        route_class: RouteClass::Proxy,
                        rule_id: Some("ruleset-1:youtube".into()),
                        rule_label: Some("YouTube".into()),
                        download: 250,
                        upload: 50,
                    },
                    FlowRecord {
                        app: "Steam.exe".into(),
                        route: "Direct".into(),
                        route_class: RouteClass::Direct,
                        rule_id: Some("ruleset-1:local".into()),
                        rule_label: Some("Local direct".into()),
                        download: 100,
                        upload: 50,
                    },
                    FlowRecord {
                        app: "Firefox.exe".into(),
                        route: "Direct".into(),
                        route_class: RouteClass::Direct,
                        rule_id: Some("ruleset-1:__everything_else__".into()),
                        rule_label: Some("Everything else".into()),
                        download: 50,
                        upload: 20,
                    },
                    FlowRecord {
                        app: "Legacy.exe".into(),
                        route: "Direct".into(),
                        route_class: RouteClass::Direct,
                        rule_id: None,
                        rule_label: None,
                        download: 80,
                        upload: 30,
                    },
                ],
            )
            .unwrap();

        let report = history.report("today", "rule", &[], now).unwrap();
        assert_eq!((report.total_download, report.total_upload), (1_000, 400));
        for label in [
            "Google",
            "YouTube",
            "Local direct",
            "Everything else",
            "Unattributed",
        ] {
            assert!(
                report.rows.iter().any(|row| row.label == label),
                "missing rule label {label}: {:?}",
                report
                    .rows
                    .iter()
                    .map(|row| row.label.as_str())
                    .collect::<Vec<_>>()
            );
        }
        assert_eq!(
            report.rows.iter().map(|row| row.total).sum::<u64>(),
            report.total
        );
        assert!(
            report
                .rows
                .iter()
                .any(|row| row.label == "Google" && row.route_class == RouteClass::Proxy)
        );
        assert!(
            report
                .rows
                .iter()
                .any(|row| row.label == "Local direct" && row.route_class == RouteClass::Direct)
        );

        let proxy = history
            .report_with_filters(
                "today",
                "rule",
                &[],
                now,
                Some(HistoryRouteFilter::Proxy),
                Some(&HistorySubjectFilter::App {
                    name: "ChatGPT.exe".into(),
                }),
            )
            .unwrap();
        assert_eq!((proxy.total_download, proxy.total_upload), (550, 150));
        assert!(
            proxy
                .rows
                .iter()
                .all(|row| row.route_class == RouteClass::Proxy)
        );
        assert!(proxy.rows.iter().any(|row| row.label == "Google"));
        assert!(proxy.rows.iter().any(|row| row.label == "YouTube"));
    }

    #[test]
    fn legacy_rows_migrate_without_losing_totals_or_route_meaning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE minute_totals (minute INTEGER PRIMARY KEY, download INTEGER NOT NULL, upload INTEGER NOT NULL, gap INTEGER NOT NULL);
             CREATE TABLE minute_apps (minute INTEGER NOT NULL, app TEXT NOT NULL, route TEXT NOT NULL, download INTEGER NOT NULL, upload INTEGER NOT NULL, PRIMARY KEY (minute, app, route));
             INSERT INTO minute_totals VALUES (1799999940, 100, 40, 0);
             INSERT INTO minute_apps VALUES (1799999940, 'A.exe', 'Direct', 10, 4);
             INSERT INTO minute_apps VALUES (1799999940, 'B.exe', 'Blocked', 20, 8);
             INSERT INTO minute_apps VALUES (1799999940, 'C.exe', 'Unattributed', 5, 2);
             INSERT INTO minute_apps VALUES (1799999940, 'D.exe', 'Falemon / Hong Kong', 30, 12);",
        )
        .unwrap();
        drop(conn);

        let history = HistoryStore::open(&path).unwrap();
        let report = history
            .report("today", "route", &[], 1_800_000_000)
            .unwrap();
        assert_eq!(report.total_download, 100);
        assert_eq!(report.total_upload, 40);
        assert!(
            report
                .rows
                .iter()
                .any(|row| row.label == "Direct" && row.route_class == RouteClass::Direct)
        );
        assert!(
            report
                .rows
                .iter()
                .any(|row| row.label == "Blocked" && row.route_class == RouteClass::Blocked)
        );
        assert!(
            report
                .rows
                .iter()
                .any(|row| row.label == "Unattributed" && row.route_class == RouteClass::Unknown)
        );
        assert!(
            report
                .rows
                .iter()
                .any(|row| row.label == "Falemon / Hong Kong"
                    && row.route_class == RouteClass::Proxy)
        );

        let rule_report = history.report("today", "rule", &[], 1_800_000_000).unwrap();
        assert_eq!(rule_report.total_download, 100);
        assert_eq!(rule_report.total_upload, 40);
        assert!(
            rule_report
                .rows
                .iter()
                .any(|row| row.label == "Unattributed")
        );
        assert_eq!(
            rule_report.rows.iter().map(|row| row.total).sum::<u64>(),
            rule_report.total,
            "legacy rows without rule identity must keep authoritative totals"
        );

        let direct = history
            .report_with_filters(
                "today",
                "route",
                &[],
                1_800_000_000,
                Some(HistoryRouteFilter::Direct),
                None,
            )
            .unwrap();
        assert_eq!((direct.total_download, direct.total_upload), (10, 4));
        assert_eq!(direct.rows.len(), 1);
        assert_eq!(direct.rows[0].route_class, RouteClass::Direct);
        assert_eq!(
            direct.bars.iter().map(|bar| bar.direct_total).sum::<u64>(),
            14
        );
        assert_eq!(
            direct.bars.iter().map(|bar| bar.proxy_total).sum::<u64>(),
            0
        );

        assert!(
            report
                .bars
                .iter()
                .all(|bar| bar.proxy_total + bar.direct_total <= bar.download + bar.upload)
        );
    }
}
