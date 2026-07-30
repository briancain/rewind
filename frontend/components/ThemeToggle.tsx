"use client";
import { useTheme } from "@/lib/theme";

export function ThemeToggle() {
  const { theme, toggle } = useTheme();
  return (
    <button
      onClick={toggle}
      aria-label={`Switch to ${theme === "dark" ? "light" : "dark"} mode`}
      className="text-lg hover:opacity-80 transition"
    >
      {theme === "dark" ? "☀️" : "🌙"}
    </button>
  );
}
