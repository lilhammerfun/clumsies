import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import MiniSearch from "minisearch";
import config from "../docs/.vitepress/config.mts";

const index = new MiniSearch({
  fields: ["text"],
  ...config.themeConfig.search.options.miniSearch?.options,
});
index.addAll(["zh/flows.md", "flows.md", "data-model.md"].map((id) => ({
  id,
  text: readFileSync(new URL(`../docs/${id}`, import.meta.url), "utf8"),
})));

for (const [query, expected] of [["回滚", "zh/flows.md"], ["rollback", "flows.md"], ["Memory", "data-model.md"]]) {
  assert(index.search(query).some(({ id }) => id === expected), `${query} must find ${expected}`);
}
console.log("Documentation search finds Chinese and English content.");
