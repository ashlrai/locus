// Single source of truth for the public site origin.
// Canonical URLs, sitemap.xml, robots.txt, and OG tags are stamped from here.
// Override for a staging host: LOCUS_SITE_ORIGIN=https://preview.example.com npm run build
export const SITE_ORIGIN = (
  process.env.LOCUS_SITE_ORIGIN || "https://locus.ashlr.ai"
).replace(/\/+$/, "");

export const SITE_NAME = "Locus";

// Repo paths cited by claim blocks resolve against this base.
export const REPO_URL = "https://github.com/ashlrai/locus";
export const REPO_BLOB = `${REPO_URL}/blob/main/`;
export const REPO_TREE = `${REPO_URL}/tree/main/`;

// Repo root relative to apps/web (for resolving cited paths on disk).
export const REPO_ROOT = new URL("../../", import.meta.url).pathname;
export const WEB_ROOT = new URL("./", import.meta.url).pathname;
