import type { Config } from 'tailwindcss'

const config: Config = {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  darkMode: 'class',
  theme: {
    extend: {
      fontFamily: {
        sans: [
          '-apple-system',
          'BlinkMacSystemFont',
          '"SF Pro Text"',
          'Inter',
          '"Microsoft YaHei UI"',
          '"PingFang SC"',
          'system-ui',
          'sans-serif',
        ],
      },
    },
  },
  plugins: [],
}

export default config
