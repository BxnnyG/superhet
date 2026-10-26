import { lazy, Suspense } from "react";
import { App } from "./App";
import { AuthLayout } from "./auth/AuthLayout";
import { usePermissions } from "./auth/context";
import { adminSectionOf, navigate, usePath } from "./router";
import { Button } from "./ui";

// The console is a separate chunk: most visitors never open it.
const AdminConsole = lazy(() => import("./admin/AdminConsole"));

/** Picks the receiver or the admin console by URL; runs inside the AuthGate. */
export function Root() {
  const path = usePath();
  const section = adminSectionOf(path);
  const perms = usePermissions();
  if (section === null) return <App />;
  if (!perms.canAdmin) {
    // Same as the server's answer to /api/v1/admin/*: no admin navigation, no data.
    return (
      <AuthLayout title="Admins only" subtitle="Your account is not allowed to open the console.">
        <Button variant="primary" size="lg" onClick={() => navigate("/")}>
          Back to the receiver
        </Button>
      </AuthLayout>
    );
  }
  return (
    <Suspense
      fallback={
        <AuthLayout title="Superhet">
          <p role="status" className="text-sm text-fg-muted">
            Loading the console…
          </p>
        </AuthLayout>
      }
    >
      <AdminConsole section={section} />
    </Suspense>
  );
}
