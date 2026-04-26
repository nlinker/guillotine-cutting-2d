#include "cutting.h"
#include <iostream>
#include <math.h>
#include <limits>

#undef max

void main() {
	// Количество детелей и сами детали
	const int PARTS_COUNT = 20;
	obj_t parts[PARTS_COUNT];

	// Количество экспериментов для каждого требуемого процента выхода
	const int EXPS_COUNT = 100;

	for (int timeout = 100; timeout < 100000; timeout*=2) {
		std::cout << "Timeout: " << timeout << std::endl;
	
		for (double factor = 0.92; factor < 0.98; factor+= 0.005) {
			// Требуемый процент выхода для карты раскроя...
			std::cout << "Factor: " << factor << "\t" << std::flush;

			// Количество успешных попыток раскроя...
			int good = 0;

			for (int i = 0; i < EXPS_COUNT; i++) {
				if (i % 10 == 0) std::cout << "." << std::flush;
				// Начальные габариты листа...
				int width = 800;
				int height = 600;

				double area = 0.0;

				for (int j = 0; j < PARTS_COUNT; j++) {
					parts[j].width = rand() % 250 + 20;
					parts[j].height = rand() % 350 + 20;
					area += parts[j].width * parts[j].height;
				}

				// Поправочный коэффициент для подгона требуемого процента выхода...
				const double scale = sqrt(area / (factor * width * height));
				width *= scale;
				height *=scale;

				// Пробуем решить...
				if (tryToCut(width, height, PARTS_COUNT, parts, timeout)) good ++;
			}

			std::cout << " Quality: " << double(good) / EXPS_COUNT << std::endl;
		}

		std::cout << std::endl;
	}


}