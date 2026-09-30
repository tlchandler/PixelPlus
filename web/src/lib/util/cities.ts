// Small offline city list for the location picker (PixelPlus works without internet).
export interface City {
	name: string;
	region: string;
	lat: number;
	lon: number;
	tz: string;
}

const raw: [string, string, number, number, string][] = [
	['New York', 'NY, USA', 40.7128, -74.006, 'America/New_York'],
	['Los Angeles', 'CA, USA', 34.0522, -118.2437, 'America/Los_Angeles'],
	['Chicago', 'IL, USA', 41.8781, -87.6298, 'America/Chicago'],
	['Houston', 'TX, USA', 29.7604, -95.3698, 'America/Chicago'],
	['Phoenix', 'AZ, USA', 33.4484, -112.074, 'America/Phoenix'],
	['Philadelphia', 'PA, USA', 39.9526, -75.1652, 'America/New_York'],
	['San Antonio', 'TX, USA', 29.4241, -98.4936, 'America/Chicago'],
	['San Diego', 'CA, USA', 32.7157, -117.1611, 'America/Los_Angeles'],
	['Dallas', 'TX, USA', 32.7767, -96.797, 'America/Chicago'],
	['Austin', 'TX, USA', 30.2672, -97.7431, 'America/Chicago'],
	['San Jose', 'CA, USA', 37.3382, -121.8863, 'America/Los_Angeles'],
	['San Francisco', 'CA, USA', 37.7749, -122.4194, 'America/Los_Angeles'],
	['Sacramento', 'CA, USA', 38.5816, -121.4944, 'America/Los_Angeles'],
	['Seattle', 'WA, USA', 47.6062, -122.3321, 'America/Los_Angeles'],
	['Portland', 'OR, USA', 45.5152, -122.6784, 'America/Los_Angeles'],
	['Denver', 'CO, USA', 39.7392, -104.9903, 'America/Denver'],
	['Salt Lake City', 'UT, USA', 40.7608, -111.891, 'America/Denver'],
	['Las Vegas', 'NV, USA', 36.1699, -115.1398, 'America/Los_Angeles'],
	['Albuquerque', 'NM, USA', 35.0844, -106.6504, 'America/Denver'],
	['Boise', 'ID, USA', 43.615, -116.2023, 'America/Boise'],
	['Boston', 'MA, USA', 42.3601, -71.0589, 'America/New_York'],
	['Washington', 'DC, USA', 38.9072, -77.0369, 'America/New_York'],
	['Baltimore', 'MD, USA', 39.2904, -76.6122, 'America/New_York'],
	['Pittsburgh', 'PA, USA', 40.4406, -79.9959, 'America/New_York'],
	['Atlanta', 'GA, USA', 33.749, -84.388, 'America/New_York'],
	['Miami', 'FL, USA', 25.7617, -80.1918, 'America/New_York'],
	['Orlando', 'FL, USA', 28.5383, -81.3792, 'America/New_York'],
	['Tampa', 'FL, USA', 27.9506, -82.4572, 'America/New_York'],
	['Jacksonville', 'FL, USA', 30.3322, -81.6557, 'America/New_York'],
	['Charlotte', 'NC, USA', 35.2271, -80.8431, 'America/New_York'],
	['Raleigh', 'NC, USA', 35.7796, -78.6382, 'America/New_York'],
	['Nashville', 'TN, USA', 36.1627, -86.7816, 'America/Chicago'],
	['Memphis', 'TN, USA', 35.1495, -90.049, 'America/Chicago'],
	['New Orleans', 'LA, USA', 29.9511, -90.0715, 'America/Chicago'],
	['Oklahoma City', 'OK, USA', 35.4676, -97.5164, 'America/Chicago'],
	['Kansas City', 'MO, USA', 39.0997, -94.5786, 'America/Chicago'],
	['St. Louis', 'MO, USA', 38.627, -90.1994, 'America/Chicago'],
	['Omaha', 'NE, USA', 41.2565, -95.9345, 'America/Chicago'],
	['Minneapolis', 'MN, USA', 44.9778, -93.265, 'America/Chicago'],
	['Milwaukee', 'WI, USA', 43.0389, -87.9065, 'America/Chicago'],
	['Madison', 'WI, USA', 43.0731, -89.4012, 'America/Chicago'],
	['Des Moines', 'IA, USA', 41.5868, -93.625, 'America/Chicago'],
	['Indianapolis', 'IN, USA', 39.7684, -86.1581, 'America/Indiana/Indianapolis'],
	['Columbus', 'OH, USA', 39.9612, -82.9988, 'America/New_York'],
	['Cleveland', 'OH, USA', 41.4993, -81.6944, 'America/New_York'],
	['Cincinnati', 'OH, USA', 39.1031, -84.512, 'America/New_York'],
	['Detroit', 'MI, USA', 42.3314, -83.0458, 'America/Detroit'],
	['Grand Rapids', 'MI, USA', 42.9634, -85.6681, 'America/Detroit'],
	['Louisville', 'KY, USA', 38.2527, -85.7585, 'America/Kentucky/Louisville'],
	['Richmond', 'VA, USA', 37.5407, -77.436, 'America/New_York'],
	['Buffalo', 'NY, USA', 42.8864, -78.8784, 'America/New_York'],
	['Albany', 'NY, USA', 42.6526, -73.7562, 'America/New_York'],
	['Hartford', 'CT, USA', 41.7658, -72.6734, 'America/New_York'],
	['Providence', 'RI, USA', 41.824, -71.4128, 'America/New_York'],
	['Portland', 'ME, USA', 43.6591, -70.2568, 'America/New_York'],
	['Burlington', 'VT, USA', 44.4759, -73.2121, 'America/New_York'],
	['Anchorage', 'AK, USA', 61.2181, -149.9003, 'America/Anchorage'],
	['Honolulu', 'HI, USA', 21.3069, -157.8583, 'Pacific/Honolulu'],
	['Toronto', 'ON, Canada', 43.6532, -79.3832, 'America/Toronto'],
	['Ottawa', 'ON, Canada', 45.4215, -75.6972, 'America/Toronto'],
	['Montreal', 'QC, Canada', 45.5017, -73.5673, 'America/Toronto'],
	['Vancouver', 'BC, Canada', 49.2827, -123.1207, 'America/Vancouver'],
	['Calgary', 'AB, Canada', 51.0447, -114.0719, 'America/Edmonton'],
	['Edmonton', 'AB, Canada', 53.5461, -113.4938, 'America/Edmonton'],
	['Winnipeg', 'MB, Canada', 49.8951, -97.1384, 'America/Winnipeg'],
	['Halifax', 'NS, Canada', 44.6488, -63.5752, 'America/Halifax'],
	['London', 'United Kingdom', 51.5074, -0.1278, 'Europe/London'],
	['Manchester', 'United Kingdom', 53.4808, -2.2426, 'Europe/London'],
	['Edinburgh', 'United Kingdom', 55.9533, -3.1883, 'Europe/London'],
	['Dublin', 'Ireland', 53.3498, -6.2603, 'Europe/Dublin'],
	['Amsterdam', 'Netherlands', 52.3676, 4.9041, 'Europe/Amsterdam'],
	['Berlin', 'Germany', 52.52, 13.405, 'Europe/Berlin'],
	['Munich', 'Germany', 48.1351, 11.582, 'Europe/Berlin'],
	['Paris', 'France', 48.8566, 2.3522, 'Europe/Paris'],
	['Brussels', 'Belgium', 50.8503, 4.3517, 'Europe/Brussels'],
	['Copenhagen', 'Denmark', 55.6761, 12.5683, 'Europe/Copenhagen'],
	['Stockholm', 'Sweden', 59.3293, 18.0686, 'Europe/Stockholm'],
	['Oslo', 'Norway', 59.9139, 10.7522, 'Europe/Oslo'],
	['Helsinki', 'Finland', 60.1699, 24.9384, 'Europe/Helsinki'],
	['Vienna', 'Austria', 48.2082, 16.3738, 'Europe/Vienna'],
	['Zurich', 'Switzerland', 47.3769, 8.5417, 'Europe/Zurich'],
	['Madrid', 'Spain', 40.4168, -3.7038, 'Europe/Madrid'],
	['Rome', 'Italy', 41.9028, 12.4964, 'Europe/Rome'],
	['Warsaw', 'Poland', 52.2297, 21.0122, 'Europe/Warsaw'],
	['Prague', 'Czechia', 50.0755, 14.4378, 'Europe/Prague'],
	['Sydney', 'NSW, Australia', -33.8688, 151.2093, 'Australia/Sydney'],
	['Melbourne', 'VIC, Australia', -37.8136, 144.9631, 'Australia/Melbourne'],
	['Brisbane', 'QLD, Australia', -27.4698, 153.0251, 'Australia/Brisbane'],
	['Perth', 'WA, Australia', -31.9505, 115.8605, 'Australia/Perth'],
	['Adelaide', 'SA, Australia', -34.9285, 138.6007, 'Australia/Adelaide'],
	['Auckland', 'New Zealand', -36.8485, 174.7633, 'Pacific/Auckland'],
	['Wellington', 'New Zealand', -41.2865, 174.7762, 'Pacific/Auckland'],
	['Mexico City', 'Mexico', 19.4326, -99.1332, 'America/Mexico_City'],
	['São Paulo', 'Brazil', -23.5505, -46.6333, 'America/Sao_Paulo'],
	['Johannesburg', 'South Africa', -26.2041, 28.0473, 'Africa/Johannesburg'],
	['Tokyo', 'Japan', 35.6762, 139.6503, 'Asia/Tokyo'],
	['Singapore', 'Singapore', 1.3521, 103.8198, 'Asia/Singapore'],
	['Manila', 'Philippines', 14.5995, 120.9842, 'Asia/Manila']
];

export const CITIES: City[] = raw.map(([name, region, lat, lon, tz]) => ({ name, region, lat, lon, tz }));

export function searchCities(q: string, limit = 8): City[] {
	const n = q.trim().toLowerCase();
	if (!n) return [];
	return CITIES.filter((c) => `${c.name} ${c.region}`.toLowerCase().includes(n)).slice(0, limit);
}

export function timezones(): string[] {
	try {
		return (Intl as any).supportedValuesOf('timeZone') as string[];
	} catch {
		return [...new Set(CITIES.map((c) => c.tz))].sort();
	}
}
