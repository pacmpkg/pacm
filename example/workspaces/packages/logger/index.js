function log(message) {
  const stamp = new Date().toISOString();
  console.log(`[lumix][${stamp}] ${message}`);
}

module.exports = { log };
