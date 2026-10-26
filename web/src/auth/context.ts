import { createContext, useContext } from "react";
import type { AuthClient, Role } from "./api";
import { FULL_ACCESS, type Permissions } from "./permissions";

export interface AuthContextValue {
  /** Login name; "Guest" for guest sessions. */
  username: string;
  userId: number | null;
  role: Role;
  kind: "user" | "guest";
  permissions: Permissions;
  client: AuthClient;
  /** End the session on the server and return to the login page. */
  signOut: () => Promise<void>;
}

export const AuthContext = createContext<AuthContextValue | null>(null);

/** Current session, or null when the app is rendered without an AuthGate (tests, storybook). */
export function useAuth(): AuthContextValue | null {
  return useContext(AuthContext);
}

/** Permissions of the session; everything is allowed without an AuthGate. */
export function usePermissions(): Permissions {
  return useContext(AuthContext)?.permissions ?? FULL_ACCESS;
}
