/** @type {import('tailwindcss').Config} */
const colors = require('tailwindcss/colors')

module.exports = {
  content: ["./*.{html,hbs}"],
  theme: {
    extend: {},
    colors: {
      ...colors,
      "damuspink": {
        600: "#D34CD9",
        500: "#F869B6",
      },
      "deeppurple": {
        700: "#BF25ED",
      }
    },
  },
  plugins: [],
}
