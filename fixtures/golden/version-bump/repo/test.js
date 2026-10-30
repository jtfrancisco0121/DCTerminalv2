const assert = require("assert");
const { shout } = require("./index");
assert.strictEqual(shout("hi"), "HI!");
assert.strictEqual(shout(""), "!");
console.log("shout: ok");
