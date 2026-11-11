import { useState } from "react";
import { describeHttpError, type Json } from "../http";
import type { NotificationsApi, Target, TargetKind } from "../notifications/api";
import {
  Alert,
  Button,
  Dialog,
  IconButton,
  NumberField,
  PasswordField,
  PlusIcon,
  Select,
  TextField,
  Toggle,
  TrashIcon,
  toasts,
} from "../ui";
import { TextArea } from "./common";

interface Header {
  name: string;
  value: string;
}

/** Ready-made webhook settings for common services; the admin still supplies the URL. */
export const PRESETS: Record<string, { label: string; config: Json }> = {
  json: {
    label: "Generic JSON",
    config: { body_template: null, content_type: "application/json", headers: [] },
  },
  ntfy: {
    label: "ntfy",
    config: {
      content_type: "text/plain",
      body_template: "{{body}}",
      headers: [
        { name: "Title", value: "{{title}}" },
        { name: "Authorization", value: "Bearer {{secret}}" },
      ],
    },
  },
  gotify: {
    label: "Gotify",
    config: {
      content_type: "application/json",
      body_template: '{"title": "{{title}}", "message": "{{body}}", "priority": 5}',
      headers: [{ name: "X-Gotify-Key", value: "{{secret}}" }],
    },
  },
};

const KIND_OPTIONS = [
  { value: "webhook", label: "Webhook" },
  { value: "email", label: "E-mail" },
  { value: "push", label: "Browser push" },
];

const asString = (v: unknown, fallback = "") => (typeof v === "string" ? v : fallback);
const asNumber = (v: unknown, fallback: number) => (typeof v === "number" ? v : fallback);

/** Create or edit a notification target. Secrets are write-only: empty means "keep". */
export function TargetEditor({
  target,
  api,
  onClose,
  onSaved,
}: {
  target: Target | null;
  api: NotificationsApi;
  onClose: () => void;
  onSaved: () => void;
}) {
  const cfg = target?.config ?? {};
  const [kind, setKind] = useState<TargetKind>(target?.kind ?? "webhook");
  const [name, setName] = useState(target?.name ?? "");
  const [url, setUrl] = useState("");
  const [secret, setSecret] = useState("");
  const [removeSecret, setRemoveSecret] = useState(false);
  const [sign, setSign] = useState(cfg.sign === true);
  const [allowPrivate, setAllowPrivate] = useState(cfg.allow_private === true);
  const [contentType, setContentType] = useState(asString(cfg.content_type, "application/json"));
  const [bodyTemplate, setBodyTemplate] = useState(asString(cfg.body_template));
  const [headers, setHeaders] = useState<Header[]>(
    Array.isArray(cfg.headers)
      ? (cfg.headers as Header[]).map((h) => ({ name: h.name, value: h.value }))
      : [],
  );
  const [timeout, setTimeoutSecs] = useState(asNumber(cfg.timeout_secs, 10));
  const [retries, setRetries] = useState(asNumber(cfg.retries, kind === "webhook" ? 4 : 3));
  const [to, setTo] = useState(Array.isArray(cfg.to) ? (cfg.to as string[]).join(", ") : "");
  const [urgency, setUrgency] = useState(asString(cfg.urgency, "normal"));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const applyPreset = (key: string) => {
    const p = PRESETS[key];
    if (!p) return;
    setContentType(asString(p.config.content_type, "application/json"));
    setBodyTemplate(asString(p.config.body_template));
    setHeaders((p.config.headers as Header[]).map((h) => ({ ...h })));
  };

  const config = (): Json => {
    if (kind === "email") {
      return {
        to: to
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean),
        retries,
      };
    }
    if (kind === "push") {
      return {
        ...(Array.isArray(cfg.user_ids) ? { user_ids: cfg.user_ids } : {}),
        urgency,
        retries,
      };
    }
    return {
      ...(url.trim() ? { url: url.trim() } : {}),
      content_type: contentType,
      body_template: bodyTemplate.trim() ? bodyTemplate : null,
      headers: headers
        .filter((h) => h.name.trim())
        .map((h) => ({ name: h.name.trim(), value: h.value })),
      timeout_secs: timeout,
      retries,
      sign,
      allow_private: allowPrivate,
    };
  };

  const save = async () => {
    if (!name.trim()) {
      setError("Give the target a name.");
      return;
    }
    if (kind === "webhook" && !target && !url.trim()) {
      setError("Enter the webhook URL.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const body = {
        name: name.trim(),
        config: config(),
        ...(kind === "webhook" && secret ? { secret } : {}),
        ...(kind === "webhook" && removeSecret && !secret ? { secret: null } : {}),
      };
      if (target) await api.updateTarget(target.id, body);
      else await api.createTarget({ ...body, kind });
      toasts.push({ tone: "success", title: target ? "Target saved" : "Target created" });
      onSaved();
    } catch (e) {
      setError(describeHttpError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog title={target ? "Edit target" : "New target"} onClose={onClose} size="lg">
      <div className="flex flex-col gap-4">
        <div className="grid gap-3 sm:grid-cols-2">
          <TextField
            label="Name"
            value={name}
            maxLength={80}
            onChange={(e) => setName(e.target.value)}
          />
          <Select
            label="Kind"
            options={KIND_OPTIONS}
            value={kind}
            disabled={target !== null}
            onChange={(k) => setKind(k as TargetKind)}
          />
        </div>

        {kind === "webhook" && (
          <>
            <Select
              label="Preset"
              options={[
                { value: "", label: "Fill from a preset…" },
                ...Object.entries(PRESETS).map(([value, p]) => ({ value, label: p.label })),
              ]}
              value=""
              onChange={applyPreset}
            />
            <TextField
              label="URL"
              value={url}
              placeholder={
                target
                  ? `${asString(cfg.url_host)}/… (kept unless you enter a new URL)`
                  : "https://"
              }
              autoComplete="off"
              onChange={(e) => setUrl(e.target.value)}
              hint="Hidden after saving because such URLs often contain tokens."
            />
            <PasswordField
              label={target?.hasSecret ? "Secret (stored; leave empty to keep)" : "Secret"}
              value={secret}
              autoComplete="new-password"
              onChange={(e) => setSecret(e.target.value)}
              hint="Signs requests (HMAC-SHA256) and fills {{secret}} in header values. Never shown again."
            />
            {target?.hasSecret && (
              <Toggle
                label="Remove the stored secret"
                checked={removeSecret}
                onChange={setRemoveSecret}
              />
            )}
            <Toggle
              label="Sign requests (X-Superhet-Signature)"
              checked={sign}
              onChange={setSign}
            />
            <div className="grid gap-3 sm:grid-cols-2">
              <TextField
                label="Content type"
                value={contentType}
                onChange={(e) => setContentType(e.target.value)}
              />
              <div className="grid grid-cols-2 gap-3">
                <NumberField
                  label="Timeout"
                  unit="s"
                  min={1}
                  max={30}
                  value={timeout}
                  onCommit={setTimeoutSecs}
                />
                <NumberField
                  label="Retries"
                  min={0}
                  max={8}
                  value={retries}
                  onCommit={setRetries}
                />
              </div>
            </div>
            <TextArea
              label="Body template (empty = standard JSON document)"
              rows={3}
              value={bodyTemplate}
              onChange={setBodyTemplate}
              hint="Placeholders: {{title}}, {{body}}, {{value}}, {{data.text}}, {{rule}} ..."
            />
            <fieldset className="flex flex-col gap-2">
              <legend className="text-xs font-medium text-fg-muted">Extra headers</legend>
              {headers.map((h, i) => (
                <div key={i} className="flex items-end gap-2">
                  <TextField
                    label="Header name"
                    className="w-44"
                    value={h.name}
                    onChange={(e) =>
                      setHeaders(
                        headers.map((x, j) => (j === i ? { ...x, name: e.target.value } : x)),
                      )
                    }
                  />
                  <TextField
                    label="Value"
                    className="flex-1"
                    value={h.value}
                    onChange={(e) =>
                      setHeaders(
                        headers.map((x, j) => (j === i ? { ...x, value: e.target.value } : x)),
                      )
                    }
                  />
                  <IconButton
                    label={`Remove header ${i + 1}`}
                    size="sm"
                    onClick={() => setHeaders(headers.filter((_, j) => j !== i))}
                  >
                    <TrashIcon />
                  </IconButton>
                </div>
              ))}
              <div>
                <Button
                  size="sm"
                  icon={<PlusIcon size={14} />}
                  onClick={() => setHeaders([...headers, { name: "", value: "" }])}
                >
                  Add header
                </Button>
              </div>
            </fieldset>
            <div className="flex flex-col gap-1">
              <Toggle
                label="Allow private network addresses"
                checked={allowPrivate}
                onChange={setAllowPrivate}
              />
              <p className="text-xs text-fg-subtle">
                Off by default: requests to loopback, LAN and link-local addresses are refused so a
                webhook cannot be aimed at internal services. Turn it on only for a service on your
                own network (for example a local ntfy or Gotify).
              </p>
            </div>
          </>
        )}

        {kind === "email" && (
          <>
            <TextField
              label="Recipients (comma separated)"
              value={to}
              onChange={(e) => setTo(e.target.value)}
              hint="The SMTP server is set with SUPERHET_SMTP_* on the worker."
            />
            <NumberField label="Retries" min={0} max={8} value={retries} onCommit={setRetries} />
          </>
        )}

        {kind === "push" && (
          <>
            <p className="text-sm text-fg-muted">
              Goes to every account that turned on browser notifications (account menu,
              Notifications).
            </p>
            <Select
              label="Urgency"
              options={["very-low", "low", "normal", "high"].map((v) => ({ value: v, label: v }))}
              value={urgency}
              onChange={setUrgency}
            />
            <NumberField label="Retries" min={0} max={8} value={retries} onCommit={setRetries} />
          </>
        )}

        {error && <Alert>{error}</Alert>}
        <div className="flex justify-end gap-2">
          <Button onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => void save()} disabled={busy}>
            {busy ? "Saving…" : "Save target"}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
