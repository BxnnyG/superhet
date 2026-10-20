import { createContext, useContext } from "react";
import type { AuthClient } from "./api";

export interface AuthContextValue {
  username: string;
  client: AuthClient;
  /** End the session on the server and return to the login page. */
  signOut: () => Promise<void>;
}

export const AuthContext = createContext<AuthContextValue | null>(null);

/** Current session, or null when the app is rendered without an AuthGate (tests, storybook). */
export function useAuth(): AuthContextValue | null {
  return useContext(AuthContext);
}
