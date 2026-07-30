import type { Config } from "jest";

const config: Config = {
  testEnvironment: "jsdom",
  transform: { "^.+\\.(ts|tsx)$": "ts-jest" },
  moduleNameMapper: { "^@/(.*)$": "<rootDir>/$1" },
  // Don't scan the Next.js build output — its standalone/package.json collides with the root
  // package.json in Jest's haste map (harmless warning otherwise).
  modulePathIgnorePatterns: ["<rootDir>/.next/"],
};

export default config;
