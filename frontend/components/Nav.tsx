"use client";
import Link from "next/link";
import { useAuth } from "@/lib/auth";
import { useState, useRef, useEffect } from "react";
import { useRouter } from "next/navigation";
import { ThemeToggle } from "@/components/ThemeToggle";

export default function Nav() {
  const { user, logout } = useAuth();
  const [query, setQuery] = useState("");
  const [dropdownOpen, setDropdownOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const router = useRouter();

  function handleSearch(e: React.FormEvent) {
    e.preventDefault();
    if (query.trim()) router.push(`/search?q=${encodeURIComponent(query)}`);
  }

  useEffect(() => {
    function handleClickOutside(e: MouseEvent) {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setDropdownOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  return (
    <nav className="bg-neutral-900 border-b border-neutral-800">
      <div className="w-full px-6 py-3 flex items-center justify-between">
        <Link href="/" className="text-xl font-bold text-red-500 shrink-0">
          Rewind<sup className="text-[10px] text-neutral-400 ml-1 font-normal">beta</sup>
        </Link>

        <form onSubmit={handleSearch} className="w-full max-w-lg mx-4">
          <input
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search videos..."
            className="w-full px-3 py-1.5 rounded bg-neutral-800 border border-neutral-700 text-sm focus:outline-none focus:border-red-500"
          />
        </form>

        <div className="flex items-center gap-4 shrink-0">
          <ThemeToggle />
          <Link href="/surf" className="text-sm bg-gradient-to-r from-purple-500 to-pink-500 px-3 py-1 rounded-full font-medium hover:opacity-90 transition">
            🏄 Surf
          </Link>
          {user ? (
            <>
              <Link href="/upload" className="text-sm text-neutral-400 hover:text-neutral-50">
                Upload
              </Link>
              <div className="relative" ref={dropdownRef}>
                <button
                  onClick={() => setDropdownOpen(!dropdownOpen)}
                  className="flex items-center gap-1 text-sm text-neutral-400 hover:text-neutral-50"
                >
                  <span className="w-7 h-7 rounded-full bg-red-600 flex items-center justify-center text-xs font-bold">
                    {(user.display_name || user.email)[0].toUpperCase()}
                  </span>
                  <span>{user.display_name || user.email}</span>
                  <span className="text-xs">▼</span>
                </button>
                {dropdownOpen && (
                  <div className="absolute right-0 mt-2 w-48 bg-neutral-800 border border-neutral-700 rounded-lg shadow-lg py-1 z-50">
                    <Link href={`/channel/${user.user_id}`} onClick={() => setDropdownOpen(false)} className="block px-4 py-2 text-sm text-neutral-400 hover:bg-neutral-700 hover:text-neutral-50">
                      My Channel
                    </Link>
                    <Link href="/history" onClick={() => setDropdownOpen(false)} className="block px-4 py-2 text-sm text-neutral-400 hover:bg-neutral-700 hover:text-neutral-50">
                      Watch History
                    </Link>
                    <Link href="/account" onClick={() => setDropdownOpen(false)} className="block px-4 py-2 text-sm text-neutral-400 hover:bg-neutral-700 hover:text-neutral-50">
                      Account
                    </Link>
                    <hr className="border-neutral-700 my-1" />
                    <button onClick={() => { setDropdownOpen(false); logout(); }} className="block w-full text-left px-4 py-2 text-sm text-neutral-400 hover:bg-neutral-700 hover:text-neutral-50">
                      Logout
                    </button>
                  </div>
                )}
              </div>
            </>
          ) : (
            <Link href="/login" className="text-sm bg-red-600 px-3 py-1 rounded hover:bg-red-700">
              Sign In
            </Link>
          )}
        </div>
      </div>
    </nav>
  );
}
