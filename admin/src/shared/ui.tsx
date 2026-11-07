import { Badge, Callout } from "@radix-ui/themes";
import { CheckCircle2, Monitor, Apple } from "lucide-react";
import { iconUrl } from "./api";
import type { CatalogApp } from "../features/catalog/schema";
export function AppIcon({ app }: { app: CatalogApp }) {
  return (
    <span className="app-icon">
      {app.icon ? (
        <img
          src={iconUrl(app.icon.path)}
          alt=""
          onError={(e) => {
            e.currentTarget.style.display = "none";
          }}
        />
      ) : (
        app.name.slice(0, 2).toUpperCase()
      )}
    </span>
  );
}
export function OS({ values }: { values: readonly string[] }) {
  return (
    <span className="os-badges">
      {values.map((v) => (
        <Badge key={v} color="gray" variant="soft">
          {v === "macos" ? <Apple size={12} /> : <Monitor size={12} />}{" "}
          {v === "macos" ? "Mac" : "Windows"}
        </Badge>
      ))}
    </span>
  );
}
export function Notice({
  error,
  success,
}: {
  error?: string;
  success?: string;
}) {
  return error ? (
    <Callout.Root color="red" role="alert">
      <Callout.Text>{error}</Callout.Text>
    </Callout.Root>
  ) : success ? (
    <Callout.Root color="green" role="status">
      <Callout.Icon>
        <CheckCircle2 size={16} />
      </Callout.Icon>
      <Callout.Text>{success}</Callout.Text>
    </Callout.Root>
  ) : null;
}
export function PageHeading({
  eyebrow,
  title,
  description,
  action,
}: {
  eyebrow: string;
  title: string;
  description: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="page-heading">
      <div>
        <span className="eyebrow">{eyebrow}</span>
        <h1>{title}</h1>
        <p>{description}</p>
      </div>
      {action}
    </div>
  );
}
