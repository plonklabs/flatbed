// Stands in for a bundler's hashed output (e.g. Vite's dist/assets/index-<hash>.js).
// Same origin as the API, so this fetch needs no CORS.
const app = document.getElementById("app");

fetch("/api/hello", {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ name: "flatbed" }),
})
  .then((res) => res.json())
  .then((data) => {
    app.textContent = data.message;
  })
  .catch((err) => {
    app.textContent = `error: ${err}`;
  });
