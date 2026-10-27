let index = 0;
let total = 0;

while (index < 1_000_000) {
  total = total + index;
  index = index + 1;
}

console.log(total);
