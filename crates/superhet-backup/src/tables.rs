//! What a backup contains: areas (what the admin ticks) and the tables behind them.
//!
//! Table and column names that end up in SQL only ever come from this static registry or from
//! the live catalogue, never from an archive.

/// A selectable part of a backup.
#[derive(Debug, Clone, Copy)]
pub struct Area {
    /// Stable id used in the API and the manifest.
    pub id: &'static str,
    /// Short English title for the UI.
    pub title: &'static str,
    /// What the area holds.
    pub description: &'static str,
    /// Included only when asked for (large or sensitive history).
    pub optional: bool,
    /// What "include credentials" adds to this area (empty: nothing).
    pub credentials: &'static str,
}

/// All areas, in import order.
pub const AREAS: &[Area] = &[
    Area {
        id: "settings",
        title: "Settings",
        description: "Guest policy, limits, audit retention, client-report policy, OIDC settings (the client secret is environment-only and never stored).",
        optional: false,
        credentials: "",
    },
    Area {
        id: "users",
        title: "Accounts",
        description: "Accounts with roles and OIDC links.",
        optional: false,
        credentials: "password hashes and open invites",
    },
    Area {
        id: "device_policies",
        title: "Device policies",
        description: "Per-device view and control rules, guest visibility, account allow lists.",
        optional: false,
        credentials: "",
    },
    Area {
        id: "bookmarks",
        title: "Bookmarks",
        description: "Bookmarks and their groups (including scanner flags).",
        optional: false,
        credentials: "",
    },
    Area {
        id: "profiles",
        title: "Profiles",
        description: "Saved receiver profiles.",
        optional: false,
        credentials: "",
    },
    Area {
        id: "channel_plans",
        title: "Channel plans",
        description: "Custom channel plans (also used as scanner channel lists).",
        optional: false,
        credentials: "",
    },
    Area {
        id: "notifications",
        title: "Notifications",
        description: "Notification rules and targets.",
        optional: false,
        credentials: "target secrets, the Web Push signing key and browser subscriptions",
    },
    Area {
        id: "plugins",
        title: "Plugin instances",
        description: "Started plugin instances and their configuration (installed plugin files live in the plugin volume).",
        optional: false,
        credentials: "",
    },
    Area {
        id: "share_links",
        title: "Share links",
        description: "Public receiver links (ADR-0029). A link only keeps working when its token hash travels, so this area is empty unless credentials are included.",
        optional: false,
        credentials: "the token hashes that make the links work",
    },
    Area {
        id: "recordings",
        title: "Recordings (metadata)",
        description: "The recordings library entries (titles, frequencies, keep flags). Interrupted recordings are restored as such.",
        optional: false,
        credentials: "",
    },
    Area {
        id: "recording_files",
        title: "Recording files",
        description: "The audio and IQ files of finished recordings (can be very large; needs the recordings metadata).",
        optional: true,
        credentials: "",
    },
    Area {
        id: "audit",
        title: "Audit log",
        description: "The audit trail (added to the existing one on import, duplicates are skipped).",
        optional: true,
        credentials: "",
    },
    Area {
        id: "decoder_events",
        title: "Decoder history",
        description: "Stored decoder events (can be large; added on import, duplicates are skipped).",
        optional: true,
        credentials: "",
    },
];

/// The areas a backup contains unless the caller picks others.
#[must_use]
pub fn default_areas() -> Vec<&'static str> {
    AREAS.iter().filter(|a| !a.optional).map(|a| a.id).collect()
}

/// Look an area up.
#[must_use]
pub fn area(id: &str) -> Option<&'static Area> {
    AREAS.iter().find(|a| a.id == id)
}

/// How a table is written back.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    /// The rows replace the table. `identity` keeps the ids of `GENERATED ALWAYS` columns.
    Plain { identity: bool },
    /// Accounts: the importing admin's own row is kept (or updated on confirmation).
    Users,
    /// Rows carry a `username` that is resolved to `user_id`; unknown names are skipped.
    ByUser,
    /// `allowed_usernames` are resolved to `allowed_users`.
    DevicePolicies,
    /// Rows are added to the table; rows matching `dedupe` (aliases `r` = new, `e` = existing)
    /// are skipped. New ids are assigned.
    Append { dedupe: &'static str },
}

/// One table in a backup.
#[derive(Debug, Clone, Copy)]
pub struct Table {
    /// Area the table belongs to.
    pub area: &'static str,
    /// Table name in schema `superhet`, also the file name `data/<name>.jsonl`.
    pub name: &'static str,
    /// Export query: one `text` column (a JSON document) per row; `$1` is "include credentials".
    pub export: &'static str,
    /// Exported only with credentials.
    pub creds_only: bool,
    /// Import behaviour.
    pub kind: Kind,
}

/// Every table, in import order (dependencies first).
pub const TABLES: &[Table] = &[
    Table {
        area: "settings",
        name: "settings",
        export: "SELECT to_jsonb(t)::text FROM superhet.settings t WHERE $1 OR true ORDER BY key",
        creds_only: false,
        kind: Kind::Plain { identity: false },
    },
    Table {
        area: "users",
        name: "users",
        export: "SELECT (CASE WHEN $1 THEN to_jsonb(u) ELSE to_jsonb(u) - 'password_hash' END - 'id')::text \
                 FROM superhet.users u ORDER BY u.id",
        creds_only: false,
        kind: Kind::Users,
    },
    Table {
        area: "users",
        name: "oidc_identities",
        export: "SELECT ((to_jsonb(i) - 'id' - 'user_id') || jsonb_build_object('username', u.username))::text \
                 FROM superhet.oidc_identities i JOIN superhet.users u ON u.id = i.user_id \
                 WHERE $1 OR true ORDER BY i.id",
        creds_only: false,
        kind: Kind::ByUser,
    },
    Table {
        area: "users",
        name: "invites",
        export: "SELECT (to_jsonb(i) - 'id' - 'created_by' - 'used_by')::text \
                 FROM superhet.invites i WHERE $1 ORDER BY i.id",
        creds_only: true,
        kind: Kind::Plain { identity: false },
    },
    Table {
        area: "device_policies",
        name: "device_policies",
        export: "SELECT ((to_jsonb(p) - 'allowed_users') || jsonb_build_object('allowed_usernames', \
                 (SELECT COALESCE(jsonb_agg(u.username ORDER BY u.username), '[]'::jsonb) \
                  FROM superhet.users u WHERE u.id = ANY (p.allowed_users))))::text \
                 FROM superhet.device_policies p WHERE $1 OR true ORDER BY p.device",
        creds_only: false,
        kind: Kind::DevicePolicies,
    },
    Table {
        area: "bookmarks",
        name: "bookmark_groups",
        export: "SELECT to_jsonb(g)::text FROM superhet.bookmark_groups g WHERE $1 OR true ORDER BY g.position, g.id",
        creds_only: false,
        kind: Kind::Plain { identity: false },
    },
    Table {
        area: "bookmarks",
        name: "bookmarks",
        export: "SELECT to_jsonb(b)::text FROM superhet.bookmarks b WHERE $1 OR true ORDER BY b.freq_hz, b.id",
        creds_only: false,
        kind: Kind::Plain { identity: false },
    },
    Table {
        area: "profiles",
        name: "profiles",
        export: "SELECT to_jsonb(p)::text FROM superhet.profiles p WHERE $1 OR true ORDER BY lower(p.name), p.id",
        creds_only: false,
        kind: Kind::Plain { identity: false },
    },
    Table {
        area: "channel_plans",
        name: "channel_plans",
        export: "SELECT to_jsonb(c)::text FROM superhet.channel_plans c WHERE $1 OR true ORDER BY lower(c.name), c.id",
        creds_only: false,
        kind: Kind::Plain { identity: false },
    },
    Table {
        area: "notifications",
        name: "notification_targets",
        export: "SELECT (CASE WHEN $1 THEN to_jsonb(t) ELSE to_jsonb(t) - 'secret' END)::text \
                 FROM superhet.notification_targets t ORDER BY t.id",
        creds_only: false,
        kind: Kind::Plain { identity: true },
    },
    Table {
        area: "notifications",
        name: "notification_rules",
        export: "SELECT to_jsonb(r)::text FROM superhet.notification_rules r WHERE $1 OR true ORDER BY r.id",
        creds_only: false,
        kind: Kind::Plain { identity: true },
    },
    Table {
        area: "notifications",
        name: "notification_rule_targets",
        export: "SELECT to_jsonb(x)::text FROM superhet.notification_rule_targets x WHERE $1 OR true \
                 ORDER BY x.rule_id, x.target_id",
        creds_only: false,
        kind: Kind::Plain { identity: false },
    },
    Table {
        area: "notifications",
        name: "notification_vapid",
        export: "SELECT to_jsonb(v)::text FROM superhet.notification_vapid v WHERE $1",
        creds_only: true,
        kind: Kind::Plain { identity: false },
    },
    Table {
        area: "notifications",
        name: "push_subscriptions",
        export: "SELECT ((to_jsonb(s) - 'id' - 'user_id') || jsonb_build_object('username', u.username))::text \
                 FROM superhet.push_subscriptions s JOIN superhet.users u ON u.id = s.user_id \
                 WHERE $1 ORDER BY s.id",
        creds_only: true,
        kind: Kind::ByUser,
    },
    Table {
        area: "plugins",
        name: "plugin_instances",
        export: "SELECT to_jsonb(p)::text FROM superhet.plugin_instances p WHERE $1 OR true ORDER BY p.id",
        creds_only: false,
        kind: Kind::Plain { identity: false },
    },
    Table {
        area: "share_links",
        name: "share_links",
        export: "SELECT to_jsonb(l)::text FROM superhet.share_links l WHERE $1 ORDER BY l.id",
        creds_only: true,
        kind: Kind::Plain { identity: true },
    },
    Table {
        area: "recordings",
        name: "recordings",
        export: "SELECT (CASE WHEN r.state = 'recording' THEN jsonb_set(to_jsonb(r), '{state}', '\"interrupted\"') \
                 ELSE to_jsonb(r) END)::text FROM superhet.recordings r WHERE $1 OR true ORDER BY r.id",
        creds_only: false,
        kind: Kind::Plain { identity: true },
    },
    Table {
        area: "audit",
        name: "audit_events",
        export: "SELECT (to_jsonb(e) - 'id')::text FROM superhet.audit_events e WHERE $1 OR true ORDER BY e.id",
        creds_only: false,
        kind: Kind::Append {
            dedupe: "e.ts = r.ts AND e.action = r.action AND e.outcome = r.outcome \
                     AND e.actor_id IS NOT DISTINCT FROM r.actor_id \
                     AND e.target IS NOT DISTINCT FROM r.target",
        },
    },
    Table {
        area: "decoder_events",
        name: "decoder_events",
        export: "SELECT (to_jsonb(e) - 'id')::text FROM superhet.decoder_events e WHERE $1 OR true ORDER BY e.id",
        creds_only: false,
        kind: Kind::Append {
            dedupe: "e.ts = r.ts AND e.device = r.device AND e.kind = r.kind AND e.value = r.value \
                     AND e.source = r.source AND e.freq_hz IS NOT DISTINCT FROM r.freq_hz",
        },
    },
];

/// The tables of the selected areas, in import order.
pub fn tables_of<'a>(areas: &'a [String]) -> impl Iterator<Item = &'static Table> + 'a {
    TABLES.iter().filter(|t| areas.iter().any(|a| a == t.area))
}

/// A registered table by name.
#[must_use]
pub fn table(name: &str) -> Option<&'static Table> {
    TABLES.iter().find(|t| t.name == name)
}
