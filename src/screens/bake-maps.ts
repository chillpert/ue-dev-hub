// Picking maps to bake lighting for, and the quality. Used by Package and by Tools > Bake lighting.
import type { MapStatus } from "../api";
import { h } from "../ui";

export interface BakeChoice {
  /// Long package names of the maps to bake.
  selected: Set<string>;
  quality: string;
}

/// Maps changed since their last bake, or never baked: what's picked to start with.
export function changedMaps(maps: MapStatus[]): Set<string> {
  return new Set(maps.filter((m) => m.kind === "changed" || m.kind === "never").map((m) => m.package));
}

/// The map list and the quality. Changes `choice` in place, then calls `changed`.
export function bakeMapPicker(maps: MapStatus[], qualities: string[], choice: BakeChoice, changed: () => void): HTMLElement[] {
  return [
    maps.length === 0
      ? h("p", { class: "muted" }, "No maps found. The build's maps come from MapsToCook in Config/DefaultGame.ini.")
      : h(
          "ul",
          { class: "map-list" },
          ...maps.map((m) =>
            h(
              "li",
              {},
              h(
                "label",
                { class: "map-row", title: m.changedFiles.length ? `Changed: ${m.changedFiles.join(", ")}` : undefined },
                h("input", {
                  type: "checkbox",
                  disabled: m.kind === "missing",
                  checked: choice.selected.has(m.package),
                  onchange: () => {
                    if (choice.selected.has(m.package)) choice.selected.delete(m.package);
                    else choice.selected.add(m.package);
                    changed();
                  },
                }),
                h("span", { class: "col grow min0" }, h("span", { class: "strong" }, m.name), h("span", { class: "muted small mono ellipsis" }, m.package)),
                h("span", { class: `pill ${m.kind}` }, m.status),
              ),
            ),
          ),
        ),
    h(
      "div",
      { class: "row between wrap gap-s" },
      h("span", { class: "muted" }, "Quality"),
      h(
        "select",
        {
          class: "select",
          "aria-label": "Lighting quality",
          onchange: (e: Event) => {
            choice.quality = (e.target as HTMLSelectElement).value;
            changed();
          },
        },
        ...qualities.map((q) => h("option", { value: q, selected: q === choice.quality }, q)),
      ),
    ),
  ];
}
