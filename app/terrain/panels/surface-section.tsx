import type { ReactNode } from "react";

export function SurfaceSection({
  children,
  description,
  name,
}: {
  children: ReactNode;
  description: string;
  name: string;
}) {
  return (
    <section className="surface-group" role="group" aria-label={name}>
      <div className="surface-group-heading">
        <strong>{name}</strong>
        <p>{description}</p>
      </div>
      {children}
    </section>
  );
}
