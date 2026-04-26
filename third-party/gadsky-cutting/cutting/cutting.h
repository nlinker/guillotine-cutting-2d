#ifdef CUTTING_EXPORTS
#define CUTTING_API __declspec(dllexport)
#else
#define CUTTING_API __declspec(dllimport)
#endif

#include <wtypes.h>

/*
 * Деталь, подлежащая раскрою
 */
struct obj_t {
	UINT width;  // initialized by client
	UINT height;
	UINT x;      // initialized by dll  
	UINT y;
	BOOL isRotated;
};

/*
 * Попытаться раскроить лист
 * Ограничение на размер деталей/листа:
 * максимальный габарит листа + максимальный габарит детали < 0xFFFFFFFF
 */
CUTTING_API BOOL tryToCut(
	UINT sheetWidth,
	UINT sheetHeight,
	UINT objectCnt,
	obj_t *pObjects,
	UINT timeout // Максимальное время выполнения в миллисекундах
);