import { Alert, Badge, type BadgeTone } from "../ui";
import {
  addrLabel,
  describeGeo,
  describeUa,
  dockerGatewayOnly,
  viaProxy,
  type AddrInfo,
  type ClientInfo,
} from "./clientInfo";

/** Documentation page about client addresses behind Docker and reverse proxies. */
export const CLIENT_ADDR_DOC =
  "https://github.com/BxnnyG/superhet/blob/main/docs/client-addresses.md";

const CLASS_TONE: Record<AddrInfo["class"], BadgeTone> = {
  public: "success",
  private: "neutral",
  docker_gateway: "warn",
  cgnat: "neutral",
  loopback: "neutral",
  link_local: "neutral",
  multicast: "neutral",
  reserved: "neutral",
};

/** Badge for the address type ("Public", "Private LAN", "Docker gateway", "IPv6 public"). */
export function AddrBadge({ info }: { info: AddrInfo | null }) {
  if (!info) return null;
  return (
    <Badge
      tone={CLASS_TONE[info.class]}
      title={
        info.class === "docker_gateway"
          ? "The Docker bridge gateway: the real client address did not reach Superhet"
          : undefined
      }
    >
      {addrLabel(info)}
    </Badge>
  );
}

/** One line: address, type badge, location. Shown in session rows. */
export function AddressLine({ ip, info }: { ip: string | null; info: ClientInfo }) {
  const geo = describeGeo(info.ipInfo);
  return (
    <span className="inline-flex flex-wrap items-center gap-x-1.5 gap-y-0.5">
      <span className="font-mono">{ip ?? "address not stored"}</span>
      <AddrBadge info={info.ipInfo} />
      {info.ipInfo?.rdns && <span className="text-fg-subtle">{info.ipInfo.rdns}</span>}
      {geo.length > 0 && <span className="text-fg-subtle">{geo.join(" · ")}</span>}
    </span>
  );
}

/** Rows for a `<dl class="grid ... sm:grid-cols-[auto_1fr]">`: address details and browser. */
export function ClientRows({
  ip,
  info,
  userAgent,
}: {
  ip: string | null;
  info: ClientInfo;
  userAgent: string | null;
}) {
  const geo = describeGeo(info.ipInfo);
  const proxied = viaProxy(ip, info.peerAddr);
  return (
    <>
      <dt className="text-fg-muted">Client address</dt>
      <dd className="flex flex-wrap items-center gap-1.5">
        <span className="font-mono">{ip ?? "not stored"}</span>
        <AddrBadge info={info.ipInfo} />
      </dd>
      {info.ipInfo?.rdns && (
        <>
          <dt className="text-fg-muted">Reverse DNS</dt>
          <dd className="font-mono break-all">{info.ipInfo.rdns}</dd>
        </>
      )}
      {geo.length > 0 && (
        <>
          <dt className="text-fg-muted">Location</dt>
          <dd>{geo.join(" · ")}</dd>
        </>
      )}
      {info.peerAddr && (
        <>
          <dt className="text-fg-muted">Connection peer</dt>
          <dd className="flex flex-wrap items-center gap-1.5">
            <span className="font-mono">{info.peerAddr}</span>
            <AddrBadge info={info.peerInfo} />
            <span className="text-fg-subtle">
              {proxied ? "forwarded by a trusted proxy" : "direct connection"}
            </span>
          </dd>
        </>
      )}
      <dt className="text-fg-muted">Browser</dt>
      <dd className="break-all">
        {describeUa(info.uaInfo, userAgent)}
        {info.uaInfo && userAgent && (info.uaInfo.browser || info.uaInfo.os) && (
          <span className="mt-0.5 block font-mono text-fg-subtle">{userAgent}</span>
        )}
      </dd>
    </>
  );
}

/**
 * Warning above the session and audit lists when every recent visitor appears to come from the
 * Docker bridge gateway, i.e. Superhet sees the NAT and not the clients.
 */
export function GatewayBanner({ infos }: { infos: readonly (AddrInfo | null)[] }) {
  if (!dockerGatewayOnly(infos)) return null;
  return (
    <Alert tone="warn">
      <strong>Real client addresses are not reaching Superhet.</strong> Every recent address is the
      Docker bridge gateway, so bans, per-address guest limits and the audit log cannot tell
      visitors apart. Run the stack with host networking or tell Superhet which reverse proxy to
      trust.{" "}
      <a className="underline" href={CLIENT_ADDR_DOC} target="_blank" rel="noreferrer noopener">
        How to fix this
      </a>
    </Alert>
  );
}
