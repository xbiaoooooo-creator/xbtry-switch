/**
 * XBTry branding and relay defaults. Keep every customer-facing name, URL and
 * default model here so a rebrand or endpoint change touches one file.
 */
export const BRAND = {
  name: "XBTry",
  website: "https://xbtry.com",
  /** Where customers create or copy their API key */
  keyUrl: "https://xbtry.com",
  /** Source repository of this desktop app (used for release notes links) */
  repo: "https://github.com/xbtry/xbtry-switch",
  claude: {
    baseUrl: "https://api.xbtry.com",
    model: "claude-opus-5-5",
  },
  codex: {
    baseUrl: "https://api.xbtry.com/v1",
    model: "gpt-6.1-sol",
  },
} as const;
