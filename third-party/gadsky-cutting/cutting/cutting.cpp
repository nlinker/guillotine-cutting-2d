#include "stdafx.h"
#include "cutting.h"

namespace cutting {

// Максимальное количество деталей, обрабатываемых алгоритмом
const unsigned MAX_PART_NUMBER = 20;

// Количество правил на одну деталь
const unsigned RULES_PER_PART = 0x8;

// Размер популяции и фактор роста
const unsigned POPULATION_SIZE = 0x100;
const unsigned POPULATE_FACTOR = 0x2;


/*
 * Деталь, подлежащая раскрою
 */
struct Part {
	// Габариты детали
	unsigned width;
	unsigned height;

	// Координаты детали после раскроя и флаг поворота
	unsigned x;
	unsigned y;
	bool isRotated;
	
	Part(unsigned width, unsigned height): width(width), height(height) {}
};

// Список деталей
typedef std::vector<Part> Parts;


// Правило резания, указатель на него и список правил
struct Rule;
typedef SmartPtr<Rule> RuleSP;
typedef std::vector<RuleSP> Rules;

/*
 * Правило резания. Определяет деталь или фрагмент карты
 */
struct Rule: public SmartBase {
	enum Type { terminal, horizontal, vertical };

	// Тип правила (терминальное, горизонтальная и вертикальная склейки)
	const Type type; 

	// Направление первого реза (горизонтальное или вертикальное)
	const Type direction; 

	// Маска определяет задействована ли соответствующая деталь в этом правиле.
	// Если n-ый бит установлен в 1, значит n-ая деталь задействована
	typedef std::bitset<MAX_PART_NUMBER> PartsMask;
	PartsMask partsMask;

	// Деталь терминального правила и флаг поворота на 90 градусов
	Part *part;
	bool isRotated;

	// Первичное и добавляемое правила "склеенного правила"
	const RuleSP primary;
	const RuleSP secondary;

	// Габариты правила (минимальные габариты, в которые этот фрагмент можно вписать) 
	unsigned width;
	unsigned height;

	// Площадь деталей в правиле и его качество (площадь деталей / площадь фрагмента)
	double partsArea;
	double quality;

	// Конструктор терминального правила
	Rule (Part *part, bool isRotated, Type direction): 
		type(terminal), 
		direction(direction),
		part(part), 
		isRotated(isRotated)
	{}

	// Фабричный метод терминальных правил
	static RuleSP createTerminal(Type direction, Part *part, unsigned partIndex, bool isRotated) {
		Rule *rule = new Rule(part, isRotated, direction);
		rule->partsMask.set(partIndex); // используется только одна деталь
		rule->width = isRotated ? part->height : part->width;
		rule->height = isRotated ? part->width : part->height;
		rule->partsArea = double(part->width) * double(part->height);
		rule->quality = rule->partsArea / (double(rule->width) * double(rule->height));
		return RuleSP(rule);
	}

	// Конструктор склееного правила
	Rule (Type type, Type direction, RuleSP primary, RuleSP secondary): 
		type(type), 
		direction(direction),
		primary(primary), 
		secondary(secondary)
	{}

	// Фабричный метод склееного правила (горизонтального)
	static RuleSP createHorizontal(Type direction, const RuleSP &primary, const RuleSP &secondary) {
		Rule *rule = new Rule(horizontal, direction, primary, secondary);
		rule->partsMask = primary->partsMask | secondary->partsMask; // детали объединяются
		rule->width = primary->width + secondary->width;
		rule->height = std::_cpp_max(primary->height, secondary->height);
		rule->partsArea = primary->partsArea + secondary->partsArea;
		rule->quality = rule->partsArea / (double(rule->width) * double(rule->height));
		return RuleSP(rule);
	}

	// Фабричный метод склееного правила (вертикального)
	static RuleSP createVertical(Type direction, const RuleSP &primary, const RuleSP &secondary) {
		Rule *rule = new Rule(vertical, direction, primary, secondary);
		rule->partsMask = primary->partsMask | secondary->partsMask; // детали объединяются
		rule->width = std::_cpp_max(primary->width, secondary->width);
		rule->height = primary->height + secondary->height;
		rule->partsArea = primary->partsArea + secondary->partsArea;
		rule->quality = rule->partsArea / (double(rule->width) * double(rule->height));
		return RuleSP(rule);
	}

	// Установить координаты деталей внутри этого правила
	void setPoints(unsigned x, unsigned y) {
		if (type == terminal) {
			part->x = x;
			part->y = y;
			part->isRotated = isRotated;
		}
		else {
			primary->setPoints(x, y);
			if (type == horizontal)	secondary->setPoints(primary->width + x, y);
			else secondary->setPoints(x, primary->height + y);
		}
	}
};


// Решение, указатель на решение и список решений
struct Solution;
typedef SmartPtr<Solution> SolutionSP;
typedef std::vector<SolutionSP> Solutions;

/*
 * Одно из решений задачи раскроя
 */
struct Solution: public SmartBase {
	// Список правил, по которым составляется карта раскроя
	Rules rules;

	// Качество решение и полнота (все ли детали использованы, см. estimate())
	double fitness;
	bool isFull;

	// Поиск наиболее подходящего правила для обрезка (width, height)
	RuleSP findMaxRule(unsigned width, unsigned height, const Rule::PartsMask &freePartsMask) const {
		double maxApplicability = double(width) * double(height);
		RuleSP maxRule;

		for (Rules::const_iterator ri = rules.begin(); ri < rules.end(); ri++) {
			// Если обрезок меньше фрагмента - пропускаем этот фрагмент...
			if (width < (*ri)->width || height < (*ri)->height) continue;

			// Если в правиле используются детали, которые уже заняты - пропускаем...
			if ((freePartsMask & (*ri)->partsMask) != (*ri)->partsMask) continue;

			// Лучшими являются фрагменты, у которых один из габаритов близок к обрезку...
			const double applicability = double(width - (*ri)->width) * double(height - (*ri)->height);
			if (applicability < maxApplicability) {
				maxApplicability = applicability;
				maxRule = *ri;
			}
		}

		return maxRule;
	}

	// Построить карту раскроя на обрезке (width, height)
	double build(
		unsigned width, 
		unsigned height, 
		Rule::PartsMask &freePartsMask, 
		unsigned x, 
		unsigned y
	) const {
		// Определяем лучшее правило для этого обрезка...
		const RuleSP &rule = findMaxRule(width, height, freePartsMask);
		if (!rule.object) return 0.0;

		// Устанавливаем координаты. (x, y) - Координаты раскраиваемого обрезка...
		rule->setPoints(x, y);
		
		// Вычеркиваем детали выбранного правила из свободных..
		freePartsMask ^= rule->partsMask;

		// Определяем габариты "нижнего-левого" и "правого-верхнего" обрезков...
		unsigned lbWidth, lbHeight, rtWidth, rtHeight;

		if (rule->direction == Rule::horizontal) {
			lbWidth = width;
			lbHeight = height - rule->height;
			rtWidth = width - rule->width;
			rtHeight = rule->height;
		}
		else {
			lbWidth = rule->width;
			lbHeight = height - rule->height;
			rtWidth = width - rule->width;
			rtHeight = height;
		}

		// Рекурсивно строим карты на обрезках...
		const double lbPartsArea = build(lbWidth, lbHeight, freePartsMask, x, y + rule->height);
		const double rtPartsArea = build(rtWidth, rtHeight, freePartsMask, x + rule->width, y);

		// Сумма деталей на трех кусках есть общая площадь деталей...
		return rule->partsArea + lbPartsArea + rtPartsArea;
	}
	
	// Оценить карту раскроя, создаваемую этим решением
	void estimate(unsigned maxWidth, unsigned maxHeight, const Parts &parts) {
		// Сначала все детали свободны... 
		Rule::PartsMask freePartsMask;
		for (unsigned i = 0; i < parts.size(); i++) freePartsMask.set(i);

		// Фитнесс-функция это площадь размещенных на карте деталей...
		fitness = build(maxWidth, maxHeight, freePartsMask, 0, 0);

		// Если маска обнулена, значит все детали были использованы...
		isFull = freePartsMask.none();
	}

	// Создать случайное решение
	static SolutionSP createRandom(Parts &parts) {
		Solution *solution = new Solution();

		for (unsigned i = 0; i < parts.size() * RULES_PER_PART; i++) {
			// Случайное решение создается из случайных терминальных правил
			const unsigned partIndex = rand() % parts.size();
			const bool isRotated = rand() % 2 != 0;
			const Rule::Type direction = rand() % 2 ? Rule::horizontal : Rule::vertical;
			const RuleSP &rule = Rule::createTerminal(direction, &parts[partIndex], partIndex, isRotated);
			solution->rules.push_back(rule);
		}

		return SolutionSP(solution);
	}

	// Создать смешанное решение из двух решений (аналог кроссовера в ГА).
	// Правила для потомка берутся либо непосредственно из предков, либо 
	// получаются путем склейки из правил предков
	static SolutionSP createPair(
		const SolutionSP &father, 
		const SolutionSP &mother, 
		unsigned maxWidth, 
		unsigned maxHeight,
		Parts &parts
	) {
		Solution *solution = new Solution();

		for (unsigned i = 0; i < father->rules.size(); i++) {
			const RuleSP &father_ = father->rules[i];
			const RuleSP &mother_ = mother->rules[rand() % mother->rules.size()];

			// Склеивание правил производится случайно, если в правилах нет общих деталей...
			bool glue = 
				rand() % 0x1 != 0 && 
				(father_->partsMask & mother_->partsMask).none();
		
			if (glue) {
				// Создаем случайное склееное правило...
				const Rule::Type direction = rand() % 2 ? Rule::horizontal : Rule::vertical;
				RuleSP rule;
				if (rand() % 2) rule = Rule::createHorizontal(direction, father_, mother_);
				else rule = Rule::createVertical(direction, father_, mother_);

				// Если габариты склейки больше максимального - не пользуемся ей...
				glue = glue && maxWidth >= rule->width && maxHeight >= rule->height;

				// Если качество склейки ниже 97% - не пользуемся ей...
				glue = glue && rule->quality >= 0.97; 
				if (glue) solution->rules.push_back(rule);
			}

			// Если склейки не получилось - берем правило от одного из предков...
			if (!glue) solution->rules.push_back(rand() % 2 ? father_ : mother_);
		}

		return SolutionSP(solution);
	}

	Solution () {
		rules.reserve(RULES_PER_PART * MAX_PART_NUMBER);
	}
};


/*
 * Алгоритм раскроя
 */
struct Solver {
	// Сравнение карт по качеству (в порядке убывания)
	struct SolutionComparator {
		bool operator () (const SolutionSP &left, const SolutionSP &right) {
			return left->fitness > right->fitness;
		}
	};

	// Список решений и исходные данные задачи
	Solutions solutions;
	unsigned maxWidth;
	unsigned maxHeight;
	Parts &parts;
	unsigned timeout;
	unsigned start;

	Solver(unsigned maxWidth, unsigned maxHeight, Parts &parts, unsigned timeout): 
		maxWidth(maxWidth),
		maxHeight(maxHeight),
		parts(parts),
		timeout(timeout),
		start(GetTickCount())
	{
		solutions.reserve(POPULATION_SIZE * POPULATE_FACTOR);
	}

	// Построить случайные решения
	void buildRandomSolutions() {
		for (unsigned i = 0; i < POPULATION_SIZE; i++) {
			const SolutionSP &solution = Solution::createRandom(parts);
			solution->estimate(maxWidth, maxHeight, parts);
			solutions.push_back(solution);
		}
	}

	// Построить смешанные решения
	void buildPairSolutions() {
		for (unsigned i = 0; i < POPULATION_SIZE * POPULATE_FACTOR; i++) {
			const unsigned fatherIndex = rand() % POPULATION_SIZE;
			const unsigned motherIndex = rand() % POPULATION_SIZE;

			const SolutionSP &solution = Solution::createPair(
				solutions[fatherIndex],
				solutions[motherIndex],
				maxWidth,
				maxHeight,
				parts
			);

			solution->estimate(maxWidth, maxHeight, parts);
			solutions.push_back(solution);
		}
	}
	
	// Основная процедура алгоритма (аналог ГА)
	bool do_() {
		buildRandomSolutions();
		std::sort(solutions.begin(), solutions.end(), SolutionComparator());

		while (true) {
			// Если превышен timeout - прекращаем поиск...
			if (GetTickCount() - start > timeout) break;

			buildPairSolutions();
			std::sort(solutions.begin(), solutions.end(), SolutionComparator());

			// Если решение полно - прекращаем поиск...
			if (solutions.front()->isFull) break;

			// Оставляем самые успешные решения...
			solutions.resize(POPULATION_SIZE);
		}

		// Вызов оценки устанавливает координаты деталей по лучшей карте...
		solutions.front()->estimate(maxWidth, maxHeight, parts);

		return solutions.front()->isFull;
	}

	// Инициализация и вызов алгоритма оптимизации
	static bool solve(unsigned sheetWidth, unsigned sheetHeight, Parts &parts, unsigned timeout) {
		Solver solver(sheetWidth, sheetHeight, parts, timeout);
		return solver.do_();
	}
};

} // namespace cutting


/*
 * Попытаться раскроить (переходная функция)
 */
CUTTING_API BOOL tryToCut(
	UINT sheetWidth,
	UINT sheetHeight,
	UINT objectCnt,
	obj_t *pObjects,
	UINT timeout
) {
	cutting::Parts parts;
	for (unsigned i = 0; i < std::_cpp_min<unsigned>(cutting::MAX_PART_NUMBER, objectCnt); i++) {
		parts.push_back(cutting::Part(pObjects[i].width, pObjects[i].height));
	}

	BOOL result = cutting::Solver::solve(sheetWidth, sheetHeight, parts, timeout);

	if (result) {
		for (i = 0; i < objectCnt; i++) {
			pObjects[i].x = parts[i].x;
			pObjects[i].y = parts[i].y;
			pObjects[i].isRotated = parts[i].isRotated;
		}
	}

	return result;
}

