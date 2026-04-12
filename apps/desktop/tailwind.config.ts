import type { Config } from "tailwindcss";

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        // CineMantis brand palette
        mantis: {
          50: "#f0fdf0",
          100: "#dcfce0",
          200: "#bbf7c1",
          300: "#86ef93",
          400: "#4ade5d",
          500: "#22c535",  // primary green
          600: "#16a324",
          700: "#15801e",
          800: "#16651d",
          900: "#14531b",
          950: "#052e0b",
        },
        surface: {
          DEFAULT: "#0f1117",
          elevated: "#161b22",
          border: "#21262d",
          hover: "#1c2128",
        },
      },
      fontFamily: {
        sans: ["Inter", "system-ui", "sans-serif"],
      },
    },
  },
  plugins: [],
} satisfies Config;
