const { log } = require("@lumix/logger");

function start() {
  log("@lumix/api booted with local workspace logger");
  return { ok: true };
}

if (require.main === module) {
  start();
  console.log("API example ran using workspace packages.");
}

module.exports = { start };
