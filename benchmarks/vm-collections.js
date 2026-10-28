const values = [0, 0, 0, 0];
const record = { total: 0 };
let index = 0;
while (index < 250_000) {
  const slot = index % 4;
  values[slot] = values[slot] + 1;
  record.total = record.total + values[slot];
  index += 1;
}
console.log(record.total);
