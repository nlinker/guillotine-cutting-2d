#include <xutility>

/*
 * Базовый класс для объектов, поддерживающих указатель со счетчиком ссылок
 */ 
struct SmartBase {
	unsigned refCount;

	SmartBase(): refCount(0) {}
};

/*
 * Указатель на объект со счетчиком ссылок
 */
template <class T> struct SmartPtr {
	T *object;

	void incRefCount() {
		if (object) object->refCount++;
	}
	
	void decRefCount() {
		if (object) {
			object->refCount--;
			if (!object->refCount) delete object;
		}
	}

	T *operator -> () const { 
		return object; 
	}

	SmartPtr(): object(NULL) {}	

	SmartPtr(T *object): object(object) { 
		incRefCount(); 
	}

	SmartPtr(const SmartPtr &pointer): object(pointer.object) { 
		incRefCount(); 
	}

	void operator = (const SmartPtr &pointer) {
		decRefCount();
		object = pointer.object;
		incRefCount();
	}

	~SmartPtr() { 
		decRefCount(); 
	}
};
