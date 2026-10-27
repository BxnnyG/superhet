import { useEffect, useMemo, type ComponentType, type MouseEvent, type ReactNode } from "react";
import { useAuth } from "../auth/context";
import { navigate } from "../router";
import {
  ActivityIcon,
  ArrowLeftIcon,
  Badge,
  BanIcon,
  Button,
  GlobeIcon,
  IconButton,
  ListIcon,
  Logo,
  MonitorIcon,
  PlugIcon,
  LogOutIcon,
  ShieldIcon,
  SlidersIcon,
  Toaster,
  UsersIcon,
  cn,
} from "../ui";
import { createPluginApi, type PluginApi } from "../plugins/api";
import { createAdminApi, type AdminApi } from "./api";
import { AuditSection } from "./AuditSection";
import { AuditSettingsSection } from "./AuditSettingsSection";
import { BansSection } from "./BansSection";
import { ClientReportSection } from "./ClientReportSection";
import { GuestSection } from "./GuestSection";
import { LimitsSection } from "./LimitsSection";
import { PluginsSection } from "./PluginsSection";
import { SessionsSection } from "./SessionsSection";
import { UsersSection } from "./UsersSection";

interface NavItem {
  id: string;
  label: string;
  Icon: ComponentType<{ size?: number; className?: string }>;
}

export const ADMIN_SECTIONS: readonly NavItem[] = [
  { id: "users", label: "Users and invites", Icon: UsersIcon },
  { id: "sessions", label: "Live sessions", Icon: ActivityIcon },
  { id: "audit", label: "Audit log", Icon: ListIcon },
  { id: "bans", label: "Address bans", Icon: BanIcon },
  { id: "plugins", label: "Plugins", Icon: PlugIcon },
  { id: "guests", label: "Guest access", Icon: GlobeIcon },
  { id: "client-info", label: "Client device info", Icon: MonitorIcon },
  { id: "audit-settings", label: "Audit settings", Icon: ShieldIcon },
  { id: "limits", label: "Limits", Icon: SlidersIcon },
];

export function sectionPath(id: string): string {
  return `/admin/${id}`;
}

/** The section for a path segment; the first section for "" and unknown names. */
export function resolveSection(segment: string): NavItem {
  return ADMIN_SECTIONS.find((s) => s.id === segment) ?? (ADMIN_SECTIONS[0] as NavItem);
}

interface Props {
  /** Path segment after `/admin/`; "" shows the first section. */
  section: string;
  api?: AdminApi;
  pluginApi?: PluginApi;
}

/**
 * Admin console at `/admin`: a sidebar of sections (a scrolling tab strip on narrow screens) and
 * the section body. Only admins get here; the server enforces it on every call regardless.
 */
export default function AdminConsole({
  section,
  api: injected,
  pluginApi: injectedPlugins,
}: Props) {
  const api = useMemo(() => injected ?? createAdminApi(), [injected]);
  const pluginApi = useMemo(() => injectedPlugins ?? createPluginApi(), [injectedPlugins]);
  const auth = useAuth();
  const current = resolveSection(section);
  // `/admin` and unknown sections canonicalise to the first section's path.
  useEffect(() => {
    if (section !== current.id) navigate(sectionPath(current.id), { replace: true });
  }, [section, current.id]);

  return (
    <div className="flex h-full flex-col overflow-hidden bg-surface-0 text-fg">
      <header className="flex h-12 shrink-0 items-center gap-3 border-b border-border bg-surface-1 px-3 shell:px-4">
        <Logo />
        <Badge tone="accent">Admin</Badge>
        <div className="flex-1" />
        <Button
          variant="secondary"
          icon={<ArrowLeftIcon size={14} />}
          onClick={() => navigate("/")}
          className="max-phone:h-11"
        >
          <span className="max-phone:sr-only">Back to receiver</span>
        </Button>
        {auth && (
          <>
            <span className="max-shell:hidden text-sm text-fg-muted" data-testid="admin-user">
              {auth.username}
            </span>
            <IconButton
              label="Sign out"
              tooltipAlign="end"
              className="max-phone:size-11"
              onClick={() => void auth.signOut()}
            >
              <LogOutIcon />
            </IconButton>
          </>
        )}
      </header>

      <div className="flex min-h-0 flex-1 flex-col shell:flex-row">
        <nav
          aria-label="Admin sections"
          className={cn(
            "shrink-0 border-b border-border bg-surface-1",
            "shell:w-60 shell:overflow-y-auto shell:border-r shell:border-b-0 shell:p-3",
          )}
        >
          <ul className="flex gap-1 overflow-x-auto px-2 py-2 shell:flex-col shell:overflow-visible shell:p-0">
            {ADMIN_SECTIONS.map(({ id, label, Icon }) => {
              const active = id === current.id;
              return (
                <li key={id} className="shrink-0">
                  <NavLink href={sectionPath(id)} active={active}>
                    <Icon size={16} className="shrink-0" />
                    <span>{label}</span>
                  </NavLink>
                </li>
              );
            })}
          </ul>
        </nav>
        <main className="min-h-0 min-w-0 flex-1 overflow-y-auto" aria-label={current.label}>
          <div className="mx-auto w-full max-w-5xl px-4 py-6 shell:px-8">
            <SectionBody
              id={current.id}
              api={api}
              pluginApi={pluginApi}
              selfId={auth?.userId ?? null}
            />
          </div>
        </main>
      </div>
      <Toaster />
    </div>
  );
}

function NavLink({
  href,
  active,
  children,
}: {
  href: string;
  active: boolean;
  children: ReactNode;
}) {
  const go = (e: MouseEvent<HTMLAnchorElement>) => {
    // Let the browser handle new-tab clicks; everything else stays in the single page app.
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
    e.preventDefault();
    navigate(href);
  };
  return (
    <a
      href={href}
      onClick={go}
      aria-current={active ? "page" : undefined}
      className={cn(
        "focus-ring flex h-9 items-center gap-2.5 rounded-md px-2.5 text-sm font-medium whitespace-nowrap",
        "transition-colors duration-150 max-phone:h-11",
        active ? "bg-accent-soft text-fg" : "text-fg-muted hover:bg-surface-2 hover:text-fg",
      )}
    >
      {children}
    </a>
  );
}

function SectionBody({
  id,
  api,
  pluginApi,
  selfId,
}: {
  id: string;
  api: AdminApi;
  pluginApi: PluginApi;
  selfId: number | null;
}) {
  switch (id) {
    case "users":
      return <UsersSection api={api} selfId={selfId} />;
    case "sessions":
      return <SessionsSection api={api} />;
    case "audit":
      return <AuditSection api={api} />;
    case "bans":
      return <BansSection api={api} />;
    case "plugins":
      return <PluginsSection api={pluginApi} />;
    case "guests":
      return <GuestSection api={api} />;
    case "client-info":
      return <ClientReportSection api={api} />;
    case "audit-settings":
      return <AuditSettingsSection api={api} />;
    case "limits":
      return <LimitsSection api={api} />;
    default:
      return null;
  }
}
