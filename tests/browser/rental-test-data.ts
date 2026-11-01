export function testRates(hourlyPrice = 100, dailyPrice = 250) {
  return {
    nonprofit: { hourly_price: hourlyPrice, weekend_day_price: dailyPrice },
    standard: { hourly_price: hourlyPrice, weekend_day_price: dailyPrice },
  }
}
