// Markdown linting — based on GitHub's ruleset (@github/markdownlint-github).
//
// Run via `mise run markdownlint` (or `markdownlint-fix`). This is the
// markdownlint-cli2 config form because GitHub's preset ships JS custom rules
// (GH001 no-default-alt-text, GH002 no-generic-link-text, GH003 no-empty-alt-text)
// that a static .markdownlint.json cannot express. Those accessibility rules
// must NOT be disabled.
//
// `init()` pulls in markdownlint's defaults plus GitHub's overrides. Add any
// project-specific overrides inside the init({ ... }) object below.
import { init } from "@github/markdownlint-github";
import markdownIt from "markdown-it";

const markdownItFactory = () => markdownIt({ html: true });

const options = {
  config: init({
    // Project overrides (GitHub defaults otherwise). Keep this list short and
    // justified — every entry is a deviation from GitHub's recommended ruleset.
    // The accessibility rules (GH001–GH003) are deliberately left untouched.
    //
    // MD013/line-length: OFF. The roadmap specs and plans use natural prose line
    // lengths (and wide tables); reflowing to 80 columns hurts readability. This
    // is the exact override GitHub's own README documents: init({'line-length': false}).
    "line-length": false,
    // MD004/ul-style: DASH. The project (and the global CLAUDE.md examples) use
    // `-` for bullets everywhere; GitHub's house style is `*`. We keep our
    // established convention rather than rewrite every document.
    "ul-style": { style: "dash" },
    // MD060/table-column-style: OFF. A fussy v0.40 rule (not configured by the
    // GitHub preset) that would force re-aligning every table pipe — cosmetic,
    // not an accessibility concern. Our tables are valid GFM.
    "table-column-style": false,
  }),
  customRules: ["@github/markdownlint-github"],
  markdownItFactory,
  outputFormatters: [["markdownlint-cli2-formatter-pretty", { appendLink: true }]],
  // Default file set for config-driven runs (`mise run markdownlint` with no args).
  globs: ["**/*.md"],
  // Never lint vendored YAML conformance docs, generated/dependency output, or
  // the local .remember memory buffer.
  ignores: [
    "crates/skald-yaml-test-suite/**",
    "target/**",
    "node_modules/**",
    ".remember/**",
  ],
};

export default options;
