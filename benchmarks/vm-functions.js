function addOne(value) {
  return value + 1;
}

let index = 0;
let total = 0;
while (index < 1_000_000) {
  total = addOne(total);
  index += 1;
}
console.log(total);
